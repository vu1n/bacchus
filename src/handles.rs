//! Handle registry for token-saving query results
//!
//! Provides a pointer system that returns compact references instead of full data.
//! Handles are session-scoped and cleared on session end.

use crate::db::with_db;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

/// Counter for generating unique handle names within a session
static HANDLE_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Flag to track if counter has been initialized from DB
static COUNTER_INITIALIZED: OnceLock<bool> = OnceLock::new();

/// Initialize counter from database max values
fn ensure_counter_initialized() {
    COUNTER_INITIALIZED.get_or_init(|| {
        // Get max handle number from database for each type
        let max_num = with_db(|conn| {
            let max: i32 = conn
                .query_row(
                    "SELECT COALESCE(MAX(CAST(SUBSTR(handle, 5) AS INTEGER)), 0) FROM handles",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(max)
        })
        .unwrap_or(0);

        // Set counter to max + 1
        if max_num > 0 {
            HANDLE_COUNTER.store((max_num + 1) as u32, Ordering::SeqCst);
        }
        true
    });
}

/// Types of handles supported
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleType {
    Symbols,
    Context,
    Messages,
}

impl HandleType {
    pub fn as_str(&self) -> &'static str {
        match self {
            HandleType::Symbols => "symbols",
            HandleType::Context => "context",
            HandleType::Messages => "messages",
        }
    }

    pub fn prefix(&self) -> &'static str {
        match self {
            HandleType::Symbols => "$sym",
            HandleType::Context => "$ctx",
            HandleType::Messages => "$msg",
        }
    }

    pub fn from_handle(handle: &str) -> Option<Self> {
        if handle.starts_with("$sym") {
            Some(HandleType::Symbols)
        } else if handle.starts_with("$ctx") {
            Some(HandleType::Context)
        } else if handle.starts_with("$msg") {
            Some(HandleType::Messages)
        } else {
            None
        }
    }
}

/// Stub returned instead of full data
#[derive(Debug, Serialize, Deserialize)]
pub struct HandleStub {
    pub handle: String,
    pub handle_type: String,
    pub count: i32,
    pub query: Option<String>,
    pub preview: Vec<String>,
}

impl std::fmt::Display for HandleStub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: Array({}) - {} items",
            self.handle, self.count, self.count
        )?;
        if !self.preview.is_empty() {
            write!(f, "\n  Preview: [{}]", self.preview.join(", "))?;
        }
        Ok(())
    }
}

/// Information about a handle for listing
#[derive(Debug, Serialize, Deserialize)]
pub struct HandleInfo {
    pub handle: String,
    pub handle_type: String,
    pub count: i32,
    pub query: Option<String>,
    pub created_at: i64,
}

/// Create a new handle storing the given data
///
/// Returns a HandleStub with the handle name and preview
pub fn create_handle(
    handle_type: HandleType,
    data: &[Value],
    query: Option<&str>,
    preview_fn: impl Fn(&Value) -> String,
) -> Result<HandleStub> {
    // Ensure counter is initialized from DB
    ensure_counter_initialized();

    let count = data.len() as i32;
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Generate unique handle name
    let num = HANDLE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let handle = format!("{}{}", handle_type.prefix(), num);

    // Generate preview (first 3 items)
    let preview: Vec<String> = data.iter().take(3).map(&preview_fn).collect();
    let preview_str = preview.join(", ");

    // Get current session ID if active
    let session_id = get_current_session_id();

    with_db(|conn| {
        // Insert handle metadata
        conn.execute(
            "INSERT INTO handles (handle, handle_type, count, query, preview, created_at, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                &handle,
                handle_type.as_str(),
                count,
                query,
                &preview_str,
                now_ms,
                session_id,
            ],
        )?;

        // Insert individual data items
        for (idx, item) in data.iter().enumerate() {
            let json = serde_json::to_string(item).unwrap_or_default();
            conn.execute(
                "INSERT INTO handle_data (handle, idx, data) VALUES (?1, ?2, ?3)",
                rusqlite::params![&handle, idx as i32, &json],
            )?;
        }

        Ok(HandleStub {
            handle,
            handle_type: handle_type.as_str().to_string(),
            count,
            query: query.map(String::from),
            preview,
        })
    })
}

/// Expand a handle to retrieve its data
pub fn expand_handle(handle: &str, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<Value>> {
    let limit = limit.unwrap_or(50);
    let offset = offset.unwrap_or(0);

    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT data FROM handle_data
             WHERE handle = ?1
             ORDER BY idx
             LIMIT ?2 OFFSET ?3",
        )?;

        let results: Vec<Value> = stmt
            .query_map(rusqlite::params![handle, limit, offset], |row| {
                let json: String = row.get(0)?;
                Ok(serde_json::from_str(&json).unwrap_or(Value::Null))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    })
}

/// Get handle metadata
pub fn get_handle_info(handle: &str) -> Result<Option<HandleInfo>> {
    with_db(|conn| {
        let result = conn.query_row(
            "SELECT handle, handle_type, count, query, created_at
             FROM handles WHERE handle = ?1",
            [handle],
            |row| {
                Ok(HandleInfo {
                    handle: row.get(0)?,
                    handle_type: row.get(1)?,
                    count: row.get(2)?,
                    query: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        );

        match result {
            Ok(info) => Ok(Some(info)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    })
}

/// Filter a handle by a predicate, creating a new handle
pub fn filter_handle(
    handle: &str,
    predicate: impl Fn(&Value) -> bool,
    preview_fn: impl Fn(&Value) -> String,
) -> Result<HandleStub> {
    // Get handle type
    let handle_type = HandleType::from_handle(handle).ok_or_else(|| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Invalid handle format: {}", handle)),
        )
    })?;

    // Get original query
    let original_query = with_db(|conn| {
        conn.query_row("SELECT query FROM handles WHERE handle = ?1", [handle], |row| {
            row.get::<_, Option<String>>(0)
        })
    })?;

    // Get all data and filter
    let all_data = expand_handle(handle, Some(10000), None)?;
    let filtered: Vec<Value> = all_data.into_iter().filter(|v| predicate(v)).collect();

    // Create new handle with filtered data
    let query = original_query.map(|q| format!("{} (filtered)", q));
    create_handle(handle_type, &filtered, query.as_deref(), preview_fn)
}

/// List all active handles
pub fn list_handles() -> Result<Vec<HandleInfo>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT handle, handle_type, count, query, created_at
             FROM handles ORDER BY created_at DESC",
        )?;

        let results: Vec<HandleInfo> = stmt
            .query_map([], |row| {
                Ok(HandleInfo {
                    handle: row.get(0)?,
                    handle_type: row.get(1)?,
                    count: row.get(2)?,
                    query: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    })
}

/// Clear all handles (manual cleanup)
pub fn clear_all_handles() -> Result<usize> {
    with_db(|conn| {
        // Delete from handle_data first due to FK
        conn.execute("DELETE FROM handle_data", [])?;
        let count = conn.execute("DELETE FROM handles", [])?;
        Ok(count)
    })
}

/// Clear handles for a specific session
pub fn clear_session_handles(session_id: &str) -> Result<usize> {
    with_db(|conn| {
        // Get handles for this session
        let mut stmt =
            conn.prepare("SELECT handle FROM handles WHERE session_id = ?1")?;
        let handles: Vec<String> = stmt
            .query_map([session_id], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        // Delete data for these handles
        for handle in &handles {
            conn.execute("DELETE FROM handle_data WHERE handle = ?1", [handle])?;
        }

        // Delete handles
        let count = conn.execute("DELETE FROM handles WHERE session_id = ?1", [session_id])?;
        Ok(count)
    })
}

/// Clear handles without session (orphaned handles)
pub fn clear_orphaned_handles() -> Result<usize> {
    with_db(|conn| {
        // Get handles without session
        let mut stmt =
            conn.prepare("SELECT handle FROM handles WHERE session_id IS NULL")?;
        let handles: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        // Delete data for these handles
        for handle in &handles {
            conn.execute("DELETE FROM handle_data WHERE handle = ?1", [handle])?;
        }

        // Delete handles
        let count = conn.execute("DELETE FROM handles WHERE session_id IS NULL", [])?;
        Ok(count)
    })
}

/// Reset the handle counter (for testing)
pub fn reset_handle_counter() {
    HANDLE_COUNTER.store(1, Ordering::SeqCst);
}

/// Get current session ID from session.json if it exists
fn get_current_session_id() -> Option<String> {
    // Try to read session file to get a unique session identifier
    let workspace_root = find_workspace_root()?;
    let session_path = workspace_root.join(".bacchus/session.json");

    if session_path.exists() {
        // Use the started_at timestamp as session identifier
        let content = std::fs::read_to_string(&session_path).ok()?;
        let session: serde_json::Value = serde_json::from_str(&content).ok()?;
        session.get("started_at")?.as_str().map(String::from)
    } else {
        None
    }
}

/// Find workspace root (duplicated from session.rs to avoid circular deps)
fn find_workspace_root() -> Option<std::path::PathBuf> {
    if let Ok(project_dir) = std::env::var("CLAUDE_PROJECT_DIR") {
        let path = std::path::PathBuf::from(&project_dir);
        if path.exists() {
            return Some(path);
        }
    }

    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join(".bacchus").exists() {
            return Some(current);
        }
        let git_path = current.join(".git");
        if git_path.exists() && git_path.is_dir() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{close_db, init_db};
    use tempfile::tempdir;

    fn setup_test_db() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap())).unwrap();
        reset_handle_counter();
        dir
    }

    #[test]
    fn test_create_handle() {
        let _dir = setup_test_db();

        let data = vec![
            serde_json::json!({"name": "foo"}),
            serde_json::json!({"name": "bar"}),
            serde_json::json!({"name": "baz"}),
        ];

        let stub = create_handle(
            HandleType::Symbols,
            &data,
            Some("test query"),
            |v| v["name"].as_str().unwrap_or("?").to_string(),
        )
        .unwrap();

        assert_eq!(stub.handle, "$sym1");
        assert_eq!(stub.count, 3);
        assert_eq!(stub.preview, vec!["foo", "bar", "baz"]);

        close_db();
    }

    #[test]
    fn test_expand_handle() {
        let _dir = setup_test_db();

        let data = vec![
            serde_json::json!({"id": 1}),
            serde_json::json!({"id": 2}),
            serde_json::json!({"id": 3}),
        ];

        let stub = create_handle(HandleType::Symbols, &data, None, |v| {
            v["id"].to_string()
        })
        .unwrap();

        let expanded = expand_handle(&stub.handle, Some(2), None).unwrap();
        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0]["id"], 1);
        assert_eq!(expanded[1]["id"], 2);

        let expanded_offset = expand_handle(&stub.handle, Some(2), Some(1)).unwrap();
        assert_eq!(expanded_offset.len(), 2);
        assert_eq!(expanded_offset[0]["id"], 2);
        assert_eq!(expanded_offset[1]["id"], 3);

        close_db();
    }

    #[test]
    fn test_filter_handle() {
        let _dir = setup_test_db();

        let data = vec![
            serde_json::json!({"kind": "function", "name": "foo"}),
            serde_json::json!({"kind": "class", "name": "Bar"}),
            serde_json::json!({"kind": "function", "name": "baz"}),
        ];

        let original = create_handle(HandleType::Symbols, &data, Some("original"), |v| {
            v["name"].as_str().unwrap_or("?").to_string()
        })
        .unwrap();

        let filtered = filter_handle(
            &original.handle,
            |v| v["kind"].as_str() == Some("function"),
            |v| v["name"].as_str().unwrap_or("?").to_string(),
        )
        .unwrap();

        assert_eq!(filtered.count, 2);
        assert!(filtered.handle.starts_with("$sym"));
        assert_ne!(filtered.handle, original.handle);

        close_db();
    }

    #[test]
    fn test_list_handles() {
        let _dir = setup_test_db();

        let data1 = vec![serde_json::json!({"a": 1})];
        let data2 = vec![serde_json::json!({"b": 2}), serde_json::json!({"c": 3})];

        create_handle(HandleType::Symbols, &data1, Some("q1"), |_| "x".to_string()).unwrap();
        create_handle(HandleType::Context, &data2, Some("q2"), |_| "y".to_string()).unwrap();

        let handles = list_handles().unwrap();
        assert_eq!(handles.len(), 2);

        close_db();
    }

    #[test]
    fn test_clear_handles() {
        let _dir = setup_test_db();

        let data = vec![serde_json::json!({"test": true})];
        create_handle(HandleType::Symbols, &data, None, |_| "t".to_string()).unwrap();

        let before = list_handles().unwrap();
        assert_eq!(before.len(), 1);

        clear_all_handles().unwrap();

        let after = list_handles().unwrap();
        assert_eq!(after.len(), 0);

        close_db();
    }
}
