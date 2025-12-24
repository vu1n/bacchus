//! Beads integration module
//!
//! Provides functions to interact with the beads issue tracking system.
//! The beads database is stored at .beads/beads.db (SQLite) relative to the workspace root.

use rusqlite::{Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

// ============================================================================
// Types
// ============================================================================

/// Information about a bead (issue)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeadInfo {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: String,
    pub status: String,
}

/// Errors that can occur when interacting with beads
#[derive(Debug, Error)]
pub enum BeadsError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Beads database not found at path: {0}")]
    DatabaseNotFound(String),

    #[error("Bead not found: {0}")]
    BeadNotFound(String),

    #[error("Invalid status: {0}")]
    InvalidStatus(String),
}

// ============================================================================
// Public API
// ============================================================================

/// Find beads that are ready to work on (status = 'open', no blocking dependencies)
pub fn get_ready_beads(workspace_root: &Path) -> Result<Vec<BeadInfo>, BeadsError> {
    let conn = open_beads_db(workspace_root)?;

    // Query for open issues that have no blocking dependencies
    let query = r#"
        SELECT i.id, i.title, i.description, i.priority, i.status
        FROM issues i
        WHERE i.status = 'open'
        AND NOT EXISTS (
            SELECT 1 FROM dependencies d
            JOIN issues blocker ON d.blocks_id = blocker.id
            WHERE d.issue_id = i.id
            AND blocker.status != 'done'
            AND blocker.status != 'closed'
        )
        ORDER BY
            CASE i.priority
                WHEN 'P0' THEN 0
                WHEN 'P1' THEN 1
                WHEN 'P2' THEN 2
                WHEN 'P3' THEN 3
                ELSE 4
            END,
            i.id
    "#;

    let mut stmt = conn.prepare(query)?;
    let beads = stmt
        .query_map([], |row| {
            Ok(BeadInfo {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                priority: row.get(3)?,
                status: row.get(4)?,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;

    Ok(beads)
}

/// Update a bead's status in the beads database
pub fn update_bead_status(
    workspace_root: &Path,
    bead_id: &str,
    status: &str,
) -> Result<(), BeadsError> {
    let conn = open_beads_db(workspace_root)?;

    let updated = conn.execute(
        "UPDATE issues SET status = ?1 WHERE id = ?2",
        [status, bead_id],
    )?;

    if updated == 0 {
        return Err(BeadsError::BeadNotFound(bead_id.to_string()));
    }

    Ok(())
}

/// Get details for a specific bead
pub fn get_bead(workspace_root: &Path, bead_id: &str) -> Result<BeadInfo, BeadsError> {
    let conn = open_beads_db(workspace_root)?;

    let bead = conn
        .query_row(
            "SELECT id, title, description, priority, status FROM issues WHERE id = ?1",
            [bead_id],
            |row| {
                Ok(BeadInfo {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    description: row.get(2)?,
                    priority: row.get(3)?,
                    status: row.get(4)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                BeadsError::BeadNotFound(bead_id.to_string())
            }
            other => BeadsError::Database(other),
        })?;

    Ok(bead)
}

// ============================================================================
// Internal Helpers
// ============================================================================

/// Open a connection to the beads database
fn open_beads_db(workspace_root: &Path) -> Result<Connection, BeadsError> {
    let db_path = workspace_root.join(".beads").join("beads.db");

    if !db_path.exists() {
        return Err(BeadsError::DatabaseNotFound(
            db_path.to_string_lossy().to_string(),
        ));
    }

    let conn = Connection::open(&db_path)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn setup_test_db(dir: &Path) -> SqlResult<()> {
        let beads_dir = dir.join(".beads");
        fs::create_dir_all(&beads_dir).unwrap();

        let db_path = beads_dir.join("beads.db");
        let conn = Connection::open(&db_path)?;

        conn.execute(
            "CREATE TABLE issues (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT,
                priority TEXT NOT NULL DEFAULT 'P2',
                status TEXT NOT NULL DEFAULT 'open'
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE dependencies (
                id INTEGER PRIMARY KEY,
                issue_id TEXT NOT NULL,
                blocks_id TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "INSERT INTO issues (id, title, priority, status) VALUES
                ('BEAD-1', 'Ready task', 'P1', 'open'),
                ('BEAD-2', 'Blocked task', 'P2', 'open'),
                ('BEAD-3', 'Blocker', 'P1', 'in_progress')",
            [],
        )?;

        conn.execute(
            "INSERT INTO dependencies (issue_id, blocks_id) VALUES ('BEAD-2', 'BEAD-3')",
            [],
        )?;

        Ok(())
    }

    #[test]
    fn test_get_ready_beads() {
        let dir = tempdir().unwrap();
        setup_test_db(dir.path()).unwrap();

        let ready = get_ready_beads(dir.path()).unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, "BEAD-1");
    }

    #[test]
    fn test_get_bead() {
        let dir = tempdir().unwrap();
        setup_test_db(dir.path()).unwrap();

        let bead = get_bead(dir.path(), "BEAD-1").unwrap();
        assert_eq!(bead.title, "Ready task");
    }

    #[test]
    fn test_update_bead_status() {
        let dir = tempdir().unwrap();
        setup_test_db(dir.path()).unwrap();

        update_bead_status(dir.path(), "BEAD-1", "in_progress").unwrap();
        let bead = get_bead(dir.path(), "BEAD-1").unwrap();
        assert_eq!(bead.status, "in_progress");
    }
}
