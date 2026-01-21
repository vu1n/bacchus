//! Database schema initialization for Bacchus
//!
//! Simple schema creation - no migrations needed for pre-v1.0.

use rusqlite::{Connection, Result};

/// Initialize the database schema
/// Creates all tables if they don't exist
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    Ok(())
}

/// Complete database schema
const SCHEMA: &str = r#"
-- ============================================================================
-- Symbols (code search)
-- ============================================================================
CREATE TABLE IF NOT EXISTS symbols (
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
CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file);
CREATE INDEX IF NOT EXISTS idx_symbols_fq_name ON symbols(fq_name);
CREATE INDEX IF NOT EXISTS idx_symbols_language ON symbols(language);
CREATE UNIQUE INDEX IF NOT EXISTS idx_symbols_file_fqname ON symbols(file, fq_name);

-- FTS5 for full-text symbol search (skip if already exists)
CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
    fq_name,
    docstring,
    content='symbols',
    content_rowid='id'
);

-- FTS sync triggers
CREATE TRIGGER IF NOT EXISTS symbols_fts_insert AFTER INSERT ON symbols BEGIN
    INSERT INTO symbols_fts(rowid, fq_name, docstring)
    VALUES (new.id, new.fq_name, COALESCE(new.docstring, ''));
END;

CREATE TRIGGER IF NOT EXISTS symbols_fts_delete AFTER DELETE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, fq_name, docstring)
    VALUES('delete', old.id, old.fq_name, COALESCE(old.docstring, ''));
END;

CREATE TRIGGER IF NOT EXISTS symbols_fts_update AFTER UPDATE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, fq_name, docstring)
    VALUES('delete', old.id, old.fq_name, COALESCE(old.docstring, ''));
    INSERT INTO symbols_fts(rowid, fq_name, docstring)
    VALUES (new.id, new.fq_name, COALESCE(new.docstring, ''));
END;

-- ============================================================================
-- Epics (high-level work containers)
-- ============================================================================
CREATE TABLE IF NOT EXISTS epics (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'open',  -- open | planning | active | closed
    created_by TEXT NOT NULL,              -- 'human' | agent_id
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (status IN ('open', 'planning', 'active', 'closed'))
);
CREATE INDEX IF NOT EXISTS idx_epics_status ON epics(status);

-- ============================================================================
-- Tasks (SQLite-based task management)
-- ============================================================================
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    epic_id TEXT NOT NULL REFERENCES epics(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    description TEXT,
    priority INTEGER NOT NULL DEFAULT 5,
    status TEXT NOT NULL DEFAULT 'draft',  -- draft | open | in_progress | ready_for_release | releasing | needs_resolution | blocked | closed
    task_type TEXT NOT NULL DEFAULT 'generic',  -- Task type for context-aware prompting
    claimed_by TEXT,                        -- agent_id who claimed
    claimed_at INTEGER,                     -- Unix timestamp ms
    ready_commit_id TEXT,                   -- jj commit ID when agent marks ready (pre-rebase)
    release_commit_id TEXT,                 -- jj commit ID after rebase (for stuck detection)
    release_started_at INTEGER,             -- When orchestrator started release attempt
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,                     -- Soft delete (NULL = active)
    CHECK (status IN ('draft', 'open', 'in_progress', 'ready_for_release', 'releasing', 'needs_resolution', 'blocked', 'closed')),
    CHECK (task_type IN ('bug_fix', 'feature', 'refactor', 'test', 'docs', 'infra', 'generic'))
);
CREATE INDEX IF NOT EXISTS idx_tasks_status_priority ON tasks(status, priority);
CREATE INDEX IF NOT EXISTS idx_tasks_epic ON tasks(epic_id);
CREATE INDEX IF NOT EXISTS idx_tasks_claimed ON tasks(claimed_by) WHERE claimed_by IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_tasks_ready ON tasks(status, priority, created_at)
    WHERE status = 'open' AND deleted_at IS NULL;

-- ============================================================================
-- Task Dependencies (many-to-many, same-epic enforced by trigger)
-- ============================================================================
CREATE TABLE IF NOT EXISTS task_dependencies (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on),
    CHECK (task_id != depends_on)
);
CREATE INDEX IF NOT EXISTS idx_task_deps_depends ON task_dependencies(depends_on);

-- ============================================================================
-- Task Footprints (normalized for overlap detection)
-- ============================================================================
CREATE TABLE IF NOT EXISTS task_footprints (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    pattern_type TEXT NOT NULL,            -- 'modifies' | 'creates'
    file_path TEXT NOT NULL,
    symbol TEXT NOT NULL DEFAULT '',
    is_wildcard INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (task_id, pattern_type, file_path, symbol),
    CHECK (pattern_type IN ('modifies', 'creates')),
    CHECK (is_wildcard IN (0, 1)),
    CHECK (is_wildcard = 0 OR symbol = ''),
    CHECK (is_wildcard = 1 OR symbol != ''),
    CHECK (pattern_type != 'creates' OR is_wildcard = 1)
);
CREATE INDEX IF NOT EXISTS idx_task_footprints_task ON task_footprints(task_id);
CREATE INDEX IF NOT EXISTS idx_task_footprints_overlap ON task_footprints(file_path, is_wildcard, symbol);

-- ============================================================================
-- Agent Messages (pull-based communication)
-- ============================================================================
CREATE TABLE IF NOT EXISTS agent_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    target_agent TEXT NOT NULL,
    message_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    processing_by TEXT,
    locked_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    processed_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_messages_target ON agent_messages(target_agent, status);
CREATE INDEX IF NOT EXISTS idx_messages_locked ON agent_messages(locked_at) WHERE status = 'processing';

-- ============================================================================
-- Triggers (only create if not exists - SQLite doesn't support IF NOT EXISTS for triggers)
-- ============================================================================

-- Same-epic dependency trigger
DROP TRIGGER IF EXISTS enforce_same_epic_deps;
CREATE TRIGGER enforce_same_epic_deps
BEFORE INSERT ON task_dependencies
BEGIN
    SELECT RAISE(ABORT, 'Dependencies must be within the same epic')
    WHERE (SELECT epic_id FROM tasks WHERE id = NEW.task_id) !=
          (SELECT epic_id FROM tasks WHERE id = NEW.depends_on);
END;

-- Auto-update timestamps
DROP TRIGGER IF EXISTS tasks_set_updated_at;
CREATE TRIGGER tasks_set_updated_at
AFTER UPDATE ON tasks
FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE tasks SET updated_at = (strftime('%s','now') * 1000) WHERE id = NEW.id;
END;

DROP TRIGGER IF EXISTS epics_set_updated_at;
CREATE TRIGGER epics_set_updated_at
AFTER UPDATE ON epics
FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE epics SET updated_at = (strftime('%s','now') * 1000) WHERE id = NEW.id;
END;

-- Block deps on deleted tasks
DROP TRIGGER IF EXISTS task_deps_guard_deleted_ins;
CREATE TRIGGER task_deps_guard_deleted_ins
BEFORE INSERT ON task_dependencies
BEGIN
    SELECT RAISE(ABORT, 'Cannot add dependency to deleted task')
    WHERE (SELECT deleted_at FROM tasks WHERE id = NEW.task_id) IS NOT NULL
       OR (SELECT deleted_at FROM tasks WHERE id = NEW.depends_on) IS NOT NULL;
END;

DROP TRIGGER IF EXISTS task_deps_guard_deleted_upd;
CREATE TRIGGER task_deps_guard_deleted_upd
BEFORE UPDATE ON task_dependencies
BEGIN
    SELECT RAISE(ABORT, 'Cannot modify dependency for deleted task')
    WHERE (SELECT deleted_at FROM tasks WHERE id = NEW.task_id) IS NOT NULL
       OR (SELECT deleted_at FROM tasks WHERE id = NEW.depends_on) IS NOT NULL;
END;

-- Block footprints on deleted tasks
DROP TRIGGER IF EXISTS task_footprints_guard_deleted_ins;
CREATE TRIGGER task_footprints_guard_deleted_ins
BEFORE INSERT ON task_footprints
BEGIN
    SELECT RAISE(ABORT, 'Cannot add footprint to deleted task')
    WHERE (SELECT deleted_at FROM tasks WHERE id = NEW.task_id) IS NOT NULL;
END;

DROP TRIGGER IF EXISTS task_footprints_guard_deleted_upd;
CREATE TRIGGER task_footprints_guard_deleted_upd
BEFORE UPDATE ON task_footprints
BEGIN
    SELECT RAISE(ABORT, 'Cannot modify footprint for deleted task')
    WHERE (SELECT deleted_at FROM tasks WHERE id = NEW.task_id) IS NOT NULL;
END;

-- ============================================================================
-- Eval Metrics (track task events for analysis)
-- ============================================================================
CREATE TABLE IF NOT EXISTS task_eval_metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    event_type TEXT NOT NULL,  -- started | completed | failed | rework | reviewed
    event_data TEXT,            -- JSON payload with details
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_eval_task ON task_eval_metrics(task_id);
CREATE INDEX IF NOT EXISTS idx_eval_agent ON task_eval_metrics(agent_id);
CREATE INDEX IF NOT EXISTS idx_eval_type ON task_eval_metrics(event_type);
CREATE INDEX IF NOT EXISTS idx_eval_time ON task_eval_metrics(created_at);

-- Soft-delete guard
DROP TRIGGER IF EXISTS tasks_soft_delete_guard;
CREATE TRIGGER tasks_soft_delete_guard
BEFORE UPDATE ON tasks
FOR EACH ROW WHEN NEW.deleted_at IS NOT NULL AND OLD.deleted_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'Deleted tasks must be closed and unclaimed')
    WHERE NEW.status != 'closed'
       OR NEW.claimed_by IS NOT NULL
       OR NEW.claimed_at IS NOT NULL;
END;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_schema() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        // Verify tables exist
        let tables = ["symbols", "epics", "tasks", "task_dependencies", "task_footprints", "agent_messages"];
        for table in tables {
            let count: i32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "Table {} should exist", table);
        }
    }

    #[test]
    fn test_init_schema_idempotent() {
        let conn = Connection::open_in_memory().unwrap();

        // Run twice - should not fail
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
    }

    #[test]
    fn test_epic_status_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        // Valid status
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
        init_schema(&conn).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Test', 'open', 'human', ?1, ?1)",
            [now],
        ).unwrap();

        // Valid status
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Test', 'draft', ?1, ?1)",
            [now],
        ).unwrap();

        // Invalid status should fail
        let result = conn.execute(
            "INSERT INTO tasks (id, epic_id, title, status, created_at, updated_at) VALUES ('T2', 'E1', 'Test', 'invalid', ?1, ?1)",
            [now],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_same_epic_dependency_trigger() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        // Two epics
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic 1', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E2', 'Epic 2', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();

        // Tasks in different epics
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task 1', 'open', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, status, created_at, updated_at) VALUES ('T2', 'E2', 'Task 2', 'open', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, status, created_at, updated_at) VALUES ('T3', 'E1', 'Task 3', 'open', ?1, ?1)",
            [now],
        ).unwrap();

        // Cross-epic should fail
        let result = conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES ('T1', 'T2')",
            [],
        );
        assert!(result.is_err());

        // Same-epic should work
        conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES ('T1', 'T3')",
            [],
        ).unwrap();
    }

    #[test]
    fn test_self_dependency_constraint() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task', 'open', ?1, ?1)",
            [now],
        ).unwrap();

        let result = conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES ('T1', 'T1')",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_footprint_constraints() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task', 'open', ?1, ?1)",
            [now],
        ).unwrap();

        // Valid modifies with symbol
        conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'modifies', 'src/auth.rs', 'AuthHandler', 0)",
            [],
        ).unwrap();

        // Valid modifies with wildcard
        conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'modifies', 'src/jwt.rs', '', 1)",
            [],
        ).unwrap();

        // Valid creates (must be wildcard)
        conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'creates', 'src/new.rs', '', 1)",
            [],
        ).unwrap();

        // Invalid: wildcard with symbol
        let result = conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'modifies', 'src/bad.rs', 'Symbol', 1)",
            [],
        );
        assert!(result.is_err());

        // Invalid: creates with non-wildcard
        let result = conn.execute(
            "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard) VALUES ('T1', 'creates', 'src/bad2.rs', '', 0)",
            [],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_soft_delete_guard() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at) VALUES ('E1', 'Epic', 'active', 'human', ?1, ?1)",
            [now],
        ).unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, status, created_at, updated_at) VALUES ('T1', 'E1', 'Task', 'open', ?1, ?1)",
            [now],
        ).unwrap();

        // Delete without closing should fail
        let result = conn.execute(
            "UPDATE tasks SET deleted_at = ?1 WHERE id = 'T1'",
            [now],
        );
        assert!(result.is_err());

        // Close then delete should work
        conn.execute("UPDATE tasks SET status = 'closed' WHERE id = 'T1'", []).unwrap();
        conn.execute("UPDATE tasks SET deleted_at = ?1 WHERE id = 'T1'", [now]).unwrap();
    }
}
