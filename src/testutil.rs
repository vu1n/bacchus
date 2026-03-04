//! Shared test utilities for creating database fixtures.
//!
//! Only compiled in test mode (`#[cfg(test)]`).

use crate::db::{init_db, with_db};
use tempfile::TempDir;

/// Create a temporary test database (empty, no fixtures).
///
/// Returns the `TempDir` guard — the database is destroyed when it drops.
pub fn setup_empty_test_db() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    init_db(Some(db_path.to_str().unwrap())).unwrap();
    dir
}

/// Create a temporary test database with a single epic and task.
///
/// Returns the `TempDir` guard — the database is destroyed when it drops.
pub fn setup_test_db(epic_id: &str, task_id: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    init_db(Some(db_path.to_str().unwrap())).unwrap();

    let now = chrono::Utc::now().timestamp_millis();
    with_db(|conn| {
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at)
             VALUES (?1, 'Test Epic', 'open', 'test', ?2, ?2)",
            rusqlite::params![epic_id, now],
        )?;
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, created_at, updated_at)
             VALUES (?1, ?2, 'Test Task', 1, 'open', 'generic', 'generic', ?3, ?3)",
            rusqlite::params![task_id, epic_id, now],
        )?;
        Ok(())
    })
    .unwrap();

    dir
}
