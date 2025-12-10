//! Database connection management for Bacchus

use rusqlite::{Connection, Result};
use std::fs;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::migrations::apply_migrations;

/// Global database connection pool (single connection for now)
static DB_POOL: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();

/// Initialize the database connection
///
/// # Arguments
/// * `db_path` - Path to the database file. If None, uses `.bacchus/bacchus.db` in current directory.
/// * `silent` - If true, suppress migration output
pub fn init_db(db_path: Option<&str>, silent: bool) -> Result<()> {
    let path = db_path.unwrap_or(".bacchus/bacchus.db");

    // Ensure directory exists
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).ok();
        }
    }

    // Create connection
    let conn = Connection::open(path)?;

    // Enable WAL mode for better concurrency
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // Enable foreign keys
    conn.pragma_update(None, "foreign_keys", "ON")?;

    // Apply migrations
    apply_migrations(&conn, silent)?;

    // Store in global pool
    let pool = DB_POOL.get_or_init(|| Mutex::new(None));
    *pool.lock().unwrap() = Some(conn);

    Ok(())
}

/// Execute a function with the database connection
pub fn with_db<F, T>(f: F) -> Result<T>
where
    F: FnOnce(&Connection) -> Result<T>,
{
    let pool = DB_POOL.get_or_init(|| Mutex::new(None));
    let guard = pool.lock().unwrap();
    let conn = guard.as_ref().ok_or_else(|| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some("Database not initialized".to_string()),
        )
    })?;
    f(conn)
}

/// Close the database connection
pub fn close_db() {
    let pool = DB_POOL.get_or_init(|| Mutex::new(None));
    *pool.lock().unwrap() = None;
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

        init_db(Some(path_str), true).unwrap();

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
