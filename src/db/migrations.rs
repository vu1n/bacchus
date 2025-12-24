//! Database migrations for Bacchus
//!
//! Migrations are numbered sequentially and applied in order.
//! Schema version is tracked in the schema_version table.

use rusqlite::{Connection, Result};

/// A database migration
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// All migrations in order
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial_schema",
        sql: r#"
-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
  version INTEGER PRIMARY KEY
);

-- Tasks table
CREATE TABLE tasks (
  bead_id        TEXT PRIMARY KEY,
  title          TEXT,
  status         TEXT,
  owner          TEXT,
  workplan       TEXT,
  footprint      TEXT,
  start_hash     TEXT,
  last_heartbeat INTEGER,
  last_update    INTEGER
);

-- Symbols table
CREATE TABLE symbols (
  id               INTEGER PRIMARY KEY,
  file             TEXT NOT NULL,
  fq_name          TEXT NOT NULL,
  kind             TEXT NOT NULL,
  span_start_line  INTEGER NOT NULL,
  span_end_line    INTEGER NOT NULL,
  line_count       INTEGER NOT NULL,
  hash             TEXT NOT NULL,
  docstring        TEXT,
  language         TEXT NOT NULL DEFAULT 'typescript'
);
CREATE INDEX idx_symbols_file ON symbols(file);
CREATE INDEX idx_symbols_fq_name ON symbols(fq_name);
CREATE INDEX idx_symbols_language ON symbols(language);

-- Bead symbols table (linking beads to symbols)
CREATE TABLE bead_symbols (
  bead_id      TEXT NOT NULL,
  symbol_ref   TEXT NOT NULL,
  symbol_id    INTEGER,
  relation     TEXT NOT NULL,
  is_virtual   INTEGER DEFAULT 0,
  PRIMARY KEY (bead_id, symbol_ref, relation)
);

-- Notifications table
CREATE TABLE notifications (
  id                  INTEGER PRIMARY KEY,
  notification_type   TEXT NOT NULL,
  from_agent          TEXT,
  from_bead           TEXT,
  commit_hash         TEXT,
  target_agent        TEXT,
  target_bead         TEXT,
  target_symbol       TEXT,
  change_kind         TEXT,
  change_description  TEXT,
  is_breaking         INTEGER DEFAULT 1,
  decision_options    TEXT,
  decision_result     TEXT,
  decision_notes      TEXT,
  status              TEXT DEFAULT 'pending',
  created_at          INTEGER NOT NULL,
  acknowledged_at     INTEGER,
  resolved_at         INTEGER
);
CREATE INDEX idx_notifications_target ON notifications(target_agent, status);
CREATE INDEX idx_notifications_symbol ON notifications(target_symbol, status);

-- Symbol calls table (for transitive dependency tracking)
CREATE TABLE symbol_calls (
  id                INTEGER PRIMARY KEY,
  caller_symbol_id  INTEGER NOT NULL,
  callee_fq_name    TEXT NOT NULL,
  call_site_file    TEXT,
  call_site_line    INTEGER,
  FOREIGN KEY (caller_symbol_id) REFERENCES symbols(id)
);
CREATE INDEX idx_symbol_calls_callee ON symbol_calls(callee_fq_name);

-- Doc fragments table
CREATE TABLE doc_fragments (
  id                TEXT PRIMARY KEY,
  path              TEXT,
  anchor            TEXT,
  scope_type        TEXT,
  scope_ref         TEXT,
  content_markdown  TEXT,
  last_generated_at INTEGER,
  stale             INTEGER DEFAULT 0
);

-- Doc sources table
CREATE TABLE doc_sources (
  fragment_id       TEXT NOT NULL,
  source_type       TEXT NOT NULL,
  source_ref        TEXT NOT NULL,
  source_hash       TEXT,
  PRIMARY KEY (fragment_id, source_type, source_ref)
);
"#,
    },
    Migration {
        version: 2,
        name: "add_parent_bead_and_estimation",
        sql: r#"
-- Add parent_bead for subtask tracking
ALTER TABLE tasks ADD COLUMN parent_bead TEXT REFERENCES tasks(bead_id);

-- Add estimation fields for auto-split decisions
ALTER TABLE tasks ADD COLUMN estimated_tokens INTEGER;
ALTER TABLE tasks ADD COLUMN estimated_files INTEGER;
ALTER TABLE tasks ADD COLUMN estimated_symbols INTEGER;

-- Index for finding subtasks
CREATE INDEX idx_tasks_parent ON tasks(parent_bead);
"#,
    },
    Migration {
        version: 3,
        name: "simplify_schema_claims_only",
        sql: r#"
-- Drop old tables we no longer need
DROP TABLE IF EXISTS notifications;
DROP TABLE IF EXISTS symbol_calls;
DROP TABLE IF EXISTS doc_fragments;
DROP TABLE IF EXISTS doc_sources;
DROP TABLE IF EXISTS bead_symbols;
DROP TABLE IF EXISTS tasks;

-- Create simplified claims table
CREATE TABLE claims (
  bead_id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL,
  worktree_path TEXT NOT NULL,
  branch_name TEXT NOT NULL,
  start_commit TEXT NOT NULL,
  claimed_at INTEGER NOT NULL
);

-- Keep symbols table as-is for code search
"#,
    },
];

/// Get the current schema version from the database
pub fn get_current_version(conn: &Connection) -> Result<i32> {
    // Try to get version, return 0 if table doesn't exist
    match conn.query_row(
        "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
        [],
        |row| row.get(0),
    ) {
        Ok(version) => Ok(version),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
        Err(rusqlite::Error::SqliteFailure(_, _)) => Ok(0), // Table doesn't exist
        Err(e) => Err(e),
    }
}

/// Apply all pending migrations
pub fn apply_migrations(conn: &Connection, silent: bool) -> Result<()> {
    let current_version = get_current_version(conn).unwrap_or(0);
    let pending: Vec<_> = MIGRATIONS
        .iter()
        .filter(|m| m.version > current_version)
        .collect();

    if pending.is_empty() {
        return Ok(());
    }

    if !silent {
        eprintln!("Applying {} migration(s)...", pending.len());
    }

    for migration in pending {
        if !silent {
            eprintln!("  Applying migration {}: {}", migration.version, migration.name);
        }

        // Execute migration in a transaction
        conn.execute_batch(migration.sql)?;

        // Update schema version
        conn.execute(
            "INSERT OR REPLACE INTO schema_version (version) VALUES (?1)",
            [migration.version],
        )?;

        if !silent {
            eprintln!("  ✓ Migration {} applied", migration.version);
        }
    }

    if !silent {
        eprintln!("All migrations applied successfully");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_migrations() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        let version = get_current_version(&conn).unwrap();
        assert_eq!(version, 3); // Update to latest migration version

        // Verify claims table exists
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='claims'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
