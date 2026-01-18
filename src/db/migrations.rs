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
    Migration {
        version: 4,
        name: "add_fts5_symbol_search",
        sql: r#"
-- Create FTS5 virtual table for full-text symbol search
CREATE VIRTUAL TABLE symbols_fts USING fts5(
    fq_name,
    docstring,
    content='symbols',
    content_rowid='id'
);

-- Populate from existing data
INSERT INTO symbols_fts(rowid, fq_name, docstring)
SELECT id, fq_name, COALESCE(docstring, '') FROM symbols;

-- Trigger to keep FTS in sync on INSERT
CREATE TRIGGER symbols_fts_insert AFTER INSERT ON symbols BEGIN
  INSERT INTO symbols_fts(rowid, fq_name, docstring)
  VALUES (new.id, new.fq_name, COALESCE(new.docstring, ''));
END;

-- Trigger to keep FTS in sync on DELETE
CREATE TRIGGER symbols_fts_delete AFTER DELETE ON symbols BEGIN
  INSERT INTO symbols_fts(symbols_fts, rowid, fq_name, docstring)
  VALUES('delete', old.id, old.fq_name, COALESCE(old.docstring, ''));
END;

-- Trigger to keep FTS in sync on UPDATE
CREATE TRIGGER symbols_fts_update AFTER UPDATE ON symbols BEGIN
  INSERT INTO symbols_fts(symbols_fts, rowid, fq_name, docstring)
  VALUES('delete', old.id, old.fq_name, COALESCE(old.docstring, ''));
  INSERT INTO symbols_fts(rowid, fq_name, docstring)
  VALUES (new.id, new.fq_name, COALESCE(new.docstring, ''));
END;
"#,
    },
    Migration {
        version: 5,
        name: "add_active_footprints",
        sql: r#"
-- Active footprints table for collision detection
-- Stores resolved footprints from active task claims
CREATE TABLE active_footprints (
    task_id TEXT NOT NULL,
    pattern TEXT NOT NULL,
    pattern_type TEXT NOT NULL,    -- 'modifies' | 'creates'
    resolved_symbols TEXT,         -- JSON array of matched fq_names
    PRIMARY KEY (task_id, pattern)
);
CREATE INDEX idx_footprints_pattern ON active_footprints(pattern);
CREATE INDEX idx_footprints_task ON active_footprints(task_id);
"#,
    },
    Migration {
        version: 6,
        name: "fix_symbols_unique_and_footprints_pk",
        sql: r#"
-- Fix symbols table: add unique constraint on (file, fq_name)
-- First, dedupe by keeping the row with max id for each (file, fq_name)
DELETE FROM symbols WHERE id NOT IN (
    SELECT MAX(id) FROM symbols GROUP BY file, fq_name
);
-- Add unique index (SQLite doesn't support ADD CONSTRAINT, use unique index)
CREATE UNIQUE INDEX IF NOT EXISTS idx_symbols_file_fqname ON symbols(file, fq_name);

-- Fix active_footprints: PK should include pattern_type
-- Same pattern can be both 'modifies' and 'creates'
DROP TABLE IF EXISTS active_footprints;
CREATE TABLE active_footprints (
    task_id TEXT NOT NULL,
    pattern TEXT NOT NULL,
    pattern_type TEXT NOT NULL,    -- 'modifies' | 'creates'
    resolved_symbols TEXT,         -- JSON array of matched fq_names
    PRIMARY KEY (task_id, pattern, pattern_type)
);
CREATE INDEX idx_footprints_pattern ON active_footprints(pattern);
CREATE INDEX idx_footprints_task ON active_footprints(task_id);

-- Update FTS triggers to handle UNIQUE constraint violations
DROP TRIGGER IF EXISTS symbols_fts_insert;
CREATE TRIGGER symbols_fts_insert AFTER INSERT ON symbols BEGIN
  INSERT INTO symbols_fts(rowid, fq_name, docstring)
  VALUES (new.id, new.fq_name, COALESCE(new.docstring, ''));
END;
"#,
    },
    Migration {
        version: 7,
        name: "hierarchical_orchestration",
        sql: r#"
-- ============================================================================
-- Migration 7: Hierarchical Orchestration with SQLite-based Tasks
-- ============================================================================
-- This migration adds support for:
-- - Epics (high-level work containers)
-- - Tasks in SQLite (replaces YAML)
-- - Task dependencies (same-epic only)
-- - Task footprints (normalized for overlap detection)
-- - Agent messages (pull-based communication)

-- ============================================================================
-- Epics table (high-level work containers)
-- ============================================================================
CREATE TABLE epics (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'open',  -- open | planning | active | closed
    created_by TEXT NOT NULL,              -- 'human' | agent_id
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (status IN ('open', 'planning', 'active', 'closed'))
);
CREATE INDEX idx_epics_status ON epics(status);

-- ============================================================================
-- Tasks table (replaces YAML tasks)
-- ============================================================================
CREATE TABLE tasks_v2 (
    id TEXT PRIMARY KEY,
    epic_id TEXT NOT NULL REFERENCES epics(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    description TEXT,
    priority INTEGER NOT NULL DEFAULT 5,
    status TEXT NOT NULL DEFAULT 'draft',  -- draft | open | in_progress | blocked | closed
    -- Claim/lease columns for atomic claiming
    claimed_by TEXT,                        -- agent_id who claimed
    claimed_at INTEGER,                     -- Unix timestamp ms
    lease_expires_at INTEGER,               -- Auto-release if heartbeat missed
    heartbeat_at INTEGER,                   -- Last heartbeat from worker
    -- Audit columns
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,                     -- Soft delete (NULL = active)
    -- Validity constraints
    CHECK (status IN ('draft', 'open', 'in_progress', 'blocked', 'closed'))
);
CREATE INDEX idx_tasks_v2_status_priority ON tasks_v2(status, priority);
CREATE INDEX idx_tasks_v2_epic ON tasks_v2(epic_id);
CREATE INDEX idx_tasks_v2_claimed ON tasks_v2(claimed_by) WHERE claimed_by IS NOT NULL;
-- Partial index for fast ready-task selection
CREATE INDEX idx_tasks_v2_ready ON tasks_v2(status, priority, created_at)
    WHERE status = 'open' AND deleted_at IS NULL;

-- ============================================================================
-- Task dependencies (many-to-many, same-epic enforced by trigger)
-- ============================================================================
CREATE TABLE task_dependencies (
    task_id TEXT NOT NULL REFERENCES tasks_v2(id) ON DELETE CASCADE,
    depends_on TEXT NOT NULL REFERENCES tasks_v2(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on),
    CHECK (task_id != depends_on)  -- Prevent self-dependency
);
CREATE INDEX idx_task_deps_depends ON task_dependencies(depends_on);

-- ============================================================================
-- Task footprints (normalized for accurate overlap detection)
-- ============================================================================
CREATE TABLE task_footprints (
    task_id TEXT NOT NULL REFERENCES tasks_v2(id) ON DELETE CASCADE,
    pattern_type TEXT NOT NULL,            -- 'modifies' | 'creates'
    file_path TEXT NOT NULL,               -- e.g., "src/auth/handler.rs"
    symbol TEXT NOT NULL DEFAULT '',       -- e.g., "AuthHandler" ('' for creates or file::*)
    is_wildcard INTEGER NOT NULL DEFAULT 0, -- 1 if file::* or creates (affects whole file)
    PRIMARY KEY (task_id, pattern_type, file_path, symbol),
    -- Validity and consistency constraints
    CHECK (pattern_type IN ('modifies', 'creates')),
    CHECK (is_wildcard IN (0, 1)),
    CHECK (is_wildcard = 0 OR symbol = ''),           -- wildcard => no symbol
    CHECK (is_wildcard = 1 OR symbol != ''),          -- non-wildcard => must have symbol
    CHECK (pattern_type != 'creates' OR is_wildcard = 1)  -- creates => wildcard
);
CREATE INDEX idx_task_footprints_task ON task_footprints(task_id);
CREATE INDEX idx_task_footprints_overlap ON task_footprints(file_path, is_wildcard, symbol);

-- ============================================================================
-- Agent messages (pull-based communication with claim semantics)
-- ============================================================================
CREATE TABLE agent_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_agent TEXT NOT NULL,            -- agent_id or 'architect'
    message_type TEXT NOT NULL,            -- 'epic_assigned' | 'breakdown_request' | etc
    payload TEXT NOT NULL,                 -- JSON
    status TEXT NOT NULL DEFAULT 'pending', -- pending | processing | processed | failed
    processing_by TEXT,                     -- agent_id that claimed this message
    locked_at INTEGER,                      -- When processing started
    attempts INTEGER NOT NULL DEFAULT 0,    -- Retry counter
    created_at INTEGER NOT NULL,
    processed_at INTEGER
);
CREATE INDEX idx_messages_target ON agent_messages(target_agent, status);
CREATE INDEX idx_messages_locked ON agent_messages(locked_at) WHERE status = 'processing';

-- ============================================================================
-- Database Constraints (Triggers)
-- ============================================================================

-- Same-epic dependency trigger (prevents cross-epic dependencies)
CREATE TRIGGER enforce_same_epic_deps
BEFORE INSERT ON task_dependencies
BEGIN
    SELECT RAISE(ABORT, 'Dependencies must be within the same epic')
    WHERE (
        SELECT epic_id FROM tasks_v2 WHERE id = NEW.task_id
    ) != (
        SELECT epic_id FROM tasks_v2 WHERE id = NEW.depends_on
    );
END;

-- Auto-update timestamp trigger for tasks (fires when caller omits updated_at)
CREATE TRIGGER tasks_v2_set_updated_at
AFTER UPDATE ON tasks_v2
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE tasks_v2
    SET updated_at = (strftime('%s','now') * 1000)
    WHERE id = NEW.id;
END;

-- Auto-update timestamp trigger for epics (fires when caller omits updated_at)
CREATE TRIGGER epics_set_updated_at
AFTER UPDATE ON epics
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE epics
    SET updated_at = (strftime('%s','now') * 1000)
    WHERE id = NEW.id;
END;

-- Block deps on deleted tasks (INSERT)
CREATE TRIGGER task_deps_guard_deleted_ins
BEFORE INSERT ON task_dependencies
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Cannot add dependency to deleted task')
    WHERE (SELECT deleted_at FROM tasks_v2 WHERE id = NEW.task_id) IS NOT NULL
       OR (SELECT deleted_at FROM tasks_v2 WHERE id = NEW.depends_on) IS NOT NULL;
END;

-- Block deps on deleted tasks (UPDATE)
CREATE TRIGGER task_deps_guard_deleted_upd
BEFORE UPDATE ON task_dependencies
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Cannot modify dependency for deleted task')
    WHERE (SELECT deleted_at FROM tasks_v2 WHERE id = NEW.task_id) IS NOT NULL
       OR (SELECT deleted_at FROM tasks_v2 WHERE id = NEW.depends_on) IS NOT NULL;
END;

-- Block footprints on deleted tasks (INSERT)
CREATE TRIGGER task_footprints_guard_deleted_ins
BEFORE INSERT ON task_footprints
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Cannot add footprint to deleted task')
    WHERE (SELECT deleted_at FROM tasks_v2 WHERE id = NEW.task_id) IS NOT NULL;
END;

-- Block footprints on deleted tasks (UPDATE)
CREATE TRIGGER task_footprints_guard_deleted_upd
BEFORE UPDATE ON task_footprints
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Cannot modify footprint for deleted task')
    WHERE (SELECT deleted_at FROM tasks_v2 WHERE id = NEW.task_id) IS NOT NULL;
END;

-- Soft-delete invariant trigger (enforce closed + unclaimed)
CREATE TRIGGER tasks_v2_soft_delete_guard
BEFORE UPDATE ON tasks_v2
FOR EACH ROW
WHEN NEW.deleted_at IS NOT NULL AND OLD.deleted_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'Deleted tasks must be closed and unclaimed')
    WHERE NEW.status != 'closed'
       OR NEW.claimed_by IS NOT NULL
       OR NEW.claimed_at IS NOT NULL
       OR NEW.lease_expires_at IS NOT NULL
       OR NEW.heartbeat_at IS NOT NULL;
END;
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
        assert_eq!(version, 7); // Update to latest migration version

        // Verify claims table exists
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='claims'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_migration_7_tables() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        // Verify epics table exists
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='epics'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify tasks_v2 table exists
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks_v2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify task_dependencies table exists
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_dependencies'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify task_footprints table exists
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_footprints'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify agent_messages table exists
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_messages'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_epic_status_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        // Valid status should work
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Test', 'open', 'human', ?1, ?1)",
            [now],
        ).unwrap();

        // Invalid status should fail
        let result = conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E2', 'Test', 'invalid', 'human', ?1, ?1)",
            [now],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_task_status_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        // Create epic first
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Test', 'open', 'human', ?1, ?1)",
            [now],
        ).unwrap();

        // Valid status should work
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Test', 'draft', ?1, ?1)",
            [now],
        ).unwrap();

        // Invalid status should fail
        let result = conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T2', 'E1', 'Test', 'invalid', ?1, ?1)",
            [now],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_same_epic_dependency_trigger() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        // Create two epics
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic 1', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E2', 'Epic 2', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();

        // Create tasks in different epics
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task 1', 'open', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T2', 'E2', 'Task 2', 'open', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T3', 'E1', 'Task 3', 'open', ?1, ?1)",
            [now],
        ).unwrap();

        // Cross-epic dependency should fail
        let result = conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES ('T1', 'T2')",
            [],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("same epic"));

        // Same-epic dependency should work
        conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES ('T1', 'T3')",
            [],
        ).unwrap();
    }

    #[test]
    fn test_self_dependency_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task', 'open', ?1, ?1)",
            [now],
        ).unwrap();

        // Self-dependency should fail
        let result = conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES ('T1', 'T1')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_footprint_constraints() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task', 'open', ?1, ?1)",
            [now],
        ).unwrap();

        // Valid modifies with specific symbol
        conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'modifies', 'src/auth.rs', 'AuthHandler', 0)",
            [],
        ).unwrap();

        // Valid modifies with wildcard (symbol must be empty)
        conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'modifies', 'src/jwt.rs', '', 1)",
            [],
        ).unwrap();

        // Valid creates (must be wildcard)
        conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'creates', 'src/new.rs', '', 1)",
            [],
        ).unwrap();

        // Invalid: wildcard with symbol should fail
        let result = conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'modifies', 'src/bad.rs', 'Symbol', 1)",
            [],
        );
        assert!(result.is_err());

        // Invalid: creates with non-wildcard should fail
        let result = conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'creates', 'src/bad2.rs', '', 0)",
            [],
        );
        assert!(result.is_err());

        // Invalid: non-wildcard with empty symbol should fail (malformed file:: pattern)
        let result = conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'modifies', 'src/bad3.rs', '', 0)",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_soft_delete_guard() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task', 'open', ?1, ?1)",
            [now],
        ).unwrap();

        // Soft-delete without closing should fail
        let result = conn.execute(
            "UPDATE tasks_v2 SET deleted_at = ?1 WHERE id = 'T1'",
            [now],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("closed and unclaimed"));

        // Close first, then soft-delete should work
        conn.execute(
            "UPDATE tasks_v2 SET status = 'closed' WHERE id = 'T1'",
            [],
        ).unwrap();
        conn.execute(
            "UPDATE tasks_v2 SET deleted_at = ?1 WHERE id = 'T1'",
            [now],
        ).unwrap();
    }

    #[test]
    fn test_deleted_task_deps_guard() {
        let conn = Connection::open_in_memory().unwrap();
        apply_migrations(&conn, true).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task 1', 'open', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks_v2 (id, epic_id, title, status, created_at, updated_at) VALUES ('T2', 'E1', 'Task 2', 'closed', ?1, ?1)",
            [now],
        ).unwrap();

        // Soft-delete T2
        conn.execute(
            "UPDATE tasks_v2 SET deleted_at = ?1 WHERE id = 'T2'",
            [now],
        ).unwrap();

        // Adding dependency to deleted task should fail
        let result = conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES ('T1', 'T2')",
            [],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("deleted task"));
    }
}
