//! Database connection management for Bacchus
//!
//! Override with BACCHUS_DB_PATH environment variable.

use rusqlite::{Connection, Result};
use std::cell::RefCell;
use std::fs;
use std::path::Path;

use super::migrations::init_schema;

thread_local! {
    /// Thread-local connection slot.
    ///
    /// This avoids cross-test interference when unit tests run concurrently.
    static DB_CONN: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

/// Initialize the database connection
///
/// # Arguments
/// * `db_path` - Path to the database file. If None, uses `.bacchus/bacchus.db` in current directory.
pub fn init_db(db_path: Option<&str>) -> Result<()> {
    let path = db_path.unwrap_or(".bacchus/bacchus.db");

    // Ensure directory exists
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).ok();
        }
    }

    // Create connection
    let conn = Connection::open(path)?;

    // Set busy timeout for concurrent access (5 seconds)
    // This makes SQLite retry instead of immediately returning SQLITE_BUSY
    conn.busy_timeout(std::time::Duration::from_secs(5))?;

    // Enable WAL mode for better concurrency
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // Enable foreign keys
    conn.pragma_update(None, "foreign_keys", "ON")?;

    // Initialize schema.
    // Concurrent process startup can race on trigger creation; treat benign
    // "trigger ... already exists" errors as success.
    if let Err(e) = init_schema(&conn) {
        let msg = e.to_string();
        if !(msg.contains("trigger") && msg.contains("already exists")) {
            return Err(e);
        }
    }

    // Store in thread-local slot
    DB_CONN.with(|slot| {
        *slot.borrow_mut() = Some(conn);
    });

    Ok(())
}

/// Execute a function with the database connection
pub fn with_db<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&Connection) -> Result<T>,
{
    DB_CONN.with(|slot| {
        let guard = slot.borrow();
        let conn = guard.as_ref().ok_or_else(|| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some("Database not initialized".to_string()),
            )
        })?;
        f(conn)
    })
}

/// Close the database connection
pub fn close_db() {
    DB_CONN.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_init_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let path_str = db_path.to_str().unwrap();

        init_db(Some(path_str)).unwrap();

        // Verify connection works
        with_db(|conn| {
            let count: i32 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )?;
            assert!(count > 0);
            Ok(())
        })
        .unwrap();

        close_db();
    }
}
