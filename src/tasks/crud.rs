//! SQLite CRUD operations for tasks.

use std::str::FromStr;

use rusqlite::params;

use crate::db::{with_db, with_db_typed};

use super::types::*;
use super::validation::normalize_footprint;

/// Execute an UPDATE and return a domain error if no rows were affected.
pub(super) fn require_affected(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
    err: TasksError,
) -> Result<(), TasksError> {
    let affected = conn.execute(sql, params)?;
    if affected == 0 {
        return Err(err);
    }
    Ok(())
}

// ============================================================================
// SQLite Task Operations (tasks)
// ============================================================================

pub(crate) const TASK_SELECT_COLUMNS: &str =
    "id, epic_id, title, description, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, ready_commit_id, release_commit_id, release_started_at, release_attempt_count, completed_at, last_activity, last_activity_at, created_at, updated_at, deleted_at";

pub(crate) fn map_sqlite_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SqliteTask> {
    let status_str: String = row.get(5)?;
    let task_type_str: String = row.get(6)?;
    Ok(SqliteTask {
        id: row.get(0)?,
        epic_id: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        priority: row.get(4)?,
        status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
        task_type: SqliteTaskType::from_str_lossy(&task_type_str),
        archetype: row.get(7)?,
        claimed_by: row.get(8)?,
        claimed_at: row.get(9)?,
        claimed_heartbeat_at: row.get(10)?,
        ready_commit_id: row.get(11)?,
        release_commit_id: row.get(12)?,
        release_started_at: row.get(13)?,
        release_attempt_count: row.get::<_, Option<i32>>(14)?.unwrap_or(0),
        completed_at: row.get(15)?,
        last_activity: row.get(16)?,
        last_activity_at: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
        deleted_at: row.get(20)?,
    })
}

/// Create a new SQLite task with dependencies and footprints
///
/// The task is created as 'draft' and atomically flipped to 'open' after
/// all dependencies and footprints are inserted.
pub fn create_sqlite_task(input: CreateSqliteTaskInput) -> Result<SqliteTask, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        // Check epic exists
        let epic_exists: bool = conn
            .query_row(
                "SELECT 1 FROM epics WHERE id = ?1",
                [&input.epic_id],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !epic_exists {
            return Err(TasksError::EpicNotFound(input.epic_id.clone()));
        }

        // Check for duplicate task ID
        let task_exists: bool = conn
            .query_row("SELECT 1 FROM tasks WHERE id = ?1", [&input.id], |_| {
                Ok(true)
            })
            .unwrap_or(false);

        if task_exists {
            return Err(TasksError::DuplicateTask(input.id.clone()));
        }

        let task_type = input
            .task_type
            .unwrap_or_else(|| infer_task_type(&input.title, input.description.as_deref()));
        let archetype = input
            .archetype
            .clone()
            .unwrap_or_else(|| "generic".to_string());

        crate::db::with_savepoint(conn, "create_task", || {
            conn.execute(
                "INSERT INTO tasks (id, epic_id, title, description, priority, status, task_type, archetype, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6, ?7, ?8, ?8)",
                params![input.id, input.epic_id, input.title, input.description, input.priority, task_type.as_str(), archetype, now],
            )?;

            for dep_id in &input.depends_on {
                conn.execute(
                    "INSERT INTO task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
                    params![input.id, dep_id],
                )?;
            }

            let normalized = normalize_footprint(&input.footprint);
            for fp in &normalized {
                conn.execute(
                    "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![input.id, fp.pattern_type, fp.file_path, fp.symbol, fp.is_wildcard as i32],
                )?;
            }

            let flip_time = chrono::Utc::now().timestamp_millis();
            conn.execute(
                "UPDATE tasks SET status = 'open', updated_at = ?2 WHERE id = ?1",
                params![input.id, flip_time],
            )?;

            Ok(SqliteTask {
                id: input.id.clone(),
                epic_id: input.epic_id.clone(),
                title: input.title.clone(),
                description: input.description.clone(),
                priority: input.priority,
                status: SqliteTaskStatus::Open,
                task_type,
                archetype: archetype.clone(),
                claimed_by: None,
                claimed_at: None,
                claimed_heartbeat_at: None,
                ready_commit_id: None,
                release_commit_id: None,
                release_started_at: None,
                release_attempt_count: 0,
                completed_at: None,
                last_activity: None,
                last_activity_at: None,
                created_at: now,
                updated_at: flip_time,
                deleted_at: None,
            })
        })
    })
}

/// Get a SQLite task by ID
pub fn get_sqlite_task(task_id: &str) -> Result<SqliteTask, TasksError> {
    with_db(|conn| {
        let sql = format!(
            "SELECT {} FROM tasks WHERE id = ?1 AND deleted_at IS NULL",
            TASK_SELECT_COLUMNS
        );
        conn.query_row(&sql, [task_id], map_sqlite_task_row)
    })
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => TasksError::TaskNotFound(task_id.to_string()),
        e => TasksError::DbError(e.to_string()),
    })
}

/// List SQLite tasks with optional filters
pub fn list_sqlite_tasks(
    epic_id: Option<&str>,
    status: Option<SqliteTaskStatus>,
    include_deleted: bool,
) -> Result<Vec<SqliteTask>, TasksError> {
    with_db(|conn| {
        let mut conditions = Vec::new();
        let mut param_values: Vec<String> = Vec::new();

        if let Some(eid) = epic_id {
            conditions.push(format!("epic_id = ?{}", param_values.len() + 1));
            param_values.push(eid.to_string());
        }

        if let Some(s) = status {
            conditions.push(format!("status = ?{}", param_values.len() + 1));
            param_values.push(s.as_str().to_string());
        }

        if !include_deleted {
            conditions.push("deleted_at IS NULL".to_string());
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT {}
             FROM tasks {} ORDER BY priority, created_at",
            TASK_SELECT_COLUMNS, where_clause
        );

        let mut stmt = conn.prepare(&sql)?;

        let params_ref: Vec<&dyn rusqlite::ToSql> = param_values
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();

        let tasks = stmt
            .query_map(params_ref.as_slice(), map_sqlite_task_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tasks)
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}

/// Update the current activity phase for an in-progress task.
///
/// Called by worker hooks to report what the agent is doing (reading, editing, testing, etc.).
/// Also refreshes the heartbeat timestamp as a side effect.
pub fn update_task_activity(
    task_id: &str,
    agent_id: &str,
    activity: &str,
) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET last_activity = ?1,
                 last_activity_at = ?2,
                 claimed_heartbeat_at = ?2,
                 updated_at = ?2
             WHERE id = ?3
               AND claimed_by = ?4
               AND status = 'in_progress'
               AND deleted_at IS NULL",
            &[&activity as &dyn rusqlite::ToSql, &now, &task_id, &agent_id],
            TasksError::TaskNotFound(format!(
                "Task {} not owned by {} or not in_progress",
                task_id, agent_id
            )),
        )
    })
}

/// Heartbeat an in-progress claim to prevent stale cleanup.
pub fn heartbeat_sqlite_task(task_id: &str, agent_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET claimed_heartbeat_at = ?1,
                 updated_at = ?1
             WHERE id = ?2
               AND claimed_by = ?3
               AND status = 'in_progress'
               AND deleted_at IS NULL",
            &[&now as &dyn rusqlite::ToSql, &task_id, &agent_id],
            TasksError::TaskNotFound(format!(
                "Task {} not owned by {} or not in_progress",
                task_id, agent_id
            )),
        )
    })
}

/// Reset a task to a status and clear claim metadata
pub fn reset_sqlite_task(task_id: &str, status: SqliteTaskStatus) -> Result<(), TasksError> {
    match status {
        SqliteTaskStatus::Open | SqliteTaskStatus::Blocked => {}
        _ => {
            return Err(TasksError::InvalidStatus(status.as_str().to_string()));
        }
    }

    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        let status_str = status.as_str();
        require_affected(
            conn,
            "UPDATE tasks
             SET status = ?1,
                 claimed_by = NULL,
                 claimed_at = NULL,
                 claimed_heartbeat_at = NULL,
                 updated_at = ?2
             WHERE id = ?3 AND deleted_at IS NULL",
            &[&status_str as &dyn rusqlite::ToSql, &now, &task_id],
            TasksError::TaskNotFound(task_id.to_string()),
        )
    })
}

// ============================================================================
// jj Workspace Release Workflow
// ============================================================================

/// Mark a task as ready for release (agent calls this when work is complete)
///
/// Stores the pre-rebase commit ID and transitions to ready_for_release status.
/// The orchestrator will later attempt to release this task.
pub fn mark_task_ready_for_release(
    task_id: &str,
    agent_id: &str,
    commit_id: &str,
) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET status = 'ready_for_release',
                 ready_commit_id = ?1,
                 updated_at = ?2
             WHERE id = ?3
               AND claimed_by = ?4
               AND status = 'in_progress'
               AND deleted_at IS NULL",
            &[
                &commit_id as &dyn rusqlite::ToSql,
                &now,
                &task_id,
                &agent_id,
            ],
            TasksError::TaskNotFound(format!(
                "Task {} not owned by {} or not in_progress",
                task_id, agent_id
            )),
        )
    })
}

/// Start releasing a task (orchestrator calls this before attempting rebase)
///
/// Transitions to releasing status and records the release start time.
pub fn start_task_release(task_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET status = 'releasing',
                 release_commit_id = NULL,
                 release_started_at = ?1,
                 release_attempt_count = 0,
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'ready_for_release'
               AND deleted_at IS NULL",
            &[&now as &dyn rusqlite::ToSql, &task_id],
            TasksError::NotReady(format!("Task {} not ready_for_release", task_id)),
        )
    })
}

/// Record the rebased commit ID while a task is in `releasing` status.
pub fn set_task_release_commit(task_id: &str, release_commit_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET release_commit_id = ?1,
                 updated_at = ?2
             WHERE id = ?3
               AND status = 'releasing'
               AND deleted_at IS NULL",
            &[&release_commit_id as &dyn rusqlite::ToSql, &now, &task_id],
            TasksError::InvalidStatus(format!("Task {} not in releasing status", task_id)),
        )
    })
}

/// Reset a release attempt back to `ready_for_release` so orchestrator can retry.
/// Kept for manual recovery; the state machine now uses escalation instead.
#[allow(dead_code)]
pub fn reset_task_release_to_ready(task_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET status = 'ready_for_release',
                 release_commit_id = NULL,
                 release_started_at = NULL,
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'releasing'
               AND deleted_at IS NULL",
            &[&now as &dyn rusqlite::ToSql, &task_id],
            TasksError::InvalidStatus(format!("Task {} not in releasing status", task_id)),
        )
    })
}

/// Complete task release successfully (orchestrator calls after advancing main)
///
/// Clears claim metadata and transitions to closed.
pub fn complete_task_release(task_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET status = 'closed',
                 completed_at = ?1,
                 claimed_by = NULL,
                 claimed_at = NULL,
                 claimed_heartbeat_at = NULL,
                 release_started_at = NULL,
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'releasing'
               AND deleted_at IS NULL",
            &[&now as &dyn rusqlite::ToSql, &task_id],
            TasksError::InvalidStatus(format!("Task {} not in releasing status", task_id)),
        )
    })
}

/// Mark task as needing resolution (orchestrator calls when conflicts occur)
///
/// Transitions to needs_resolution status. Agent or human must resolve conflicts.
pub fn mark_task_needs_resolution(
    task_id: &str,
    _conflict_files: &[String],
) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET status = 'needs_resolution',
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'releasing'
               AND deleted_at IS NULL",
            &[&now as &dyn rusqlite::ToSql, &task_id],
            TasksError::InvalidStatus(format!("Task {} not in releasing status", task_id)),
        )
    })
}

/// Get tasks ready for release (orchestrator uses this to find work)
pub fn get_tasks_ready_for_release() -> Result<Vec<SqliteTask>, TasksError> {
    with_db(|conn| {
        let sql = format!(
            "SELECT {} FROM tasks
             WHERE status = 'ready_for_release'
               AND deleted_at IS NULL
             ORDER BY priority, created_at",
            TASK_SELECT_COLUMNS
        );
        let mut stmt = conn.prepare(&sql)?;

        let tasks = stmt
            .query_map([], map_sqlite_task_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tasks)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Increment the release attempt counter for a task in `releasing` status.
pub fn increment_release_attempt_count(task_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET release_attempt_count = release_attempt_count + 1,
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'releasing'
               AND deleted_at IS NULL",
            &[&now as &dyn rusqlite::ToSql, &task_id],
            TasksError::InvalidStatus(format!("Task {} not in releasing status", task_id)),
        )
    })
}

/// Escalate a releasing task to needs_resolution with a reason.
pub fn escalate_releasing_task(task_id: &str, reason: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET status = 'needs_resolution',
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'releasing'
               AND deleted_at IS NULL",
            &[&now as &dyn rusqlite::ToSql, &task_id],
            TasksError::InvalidStatus(format!(
                "Task {} not in releasing status (escalation: {})",
                task_id, reason
            )),
        )
    })
}

/// Reset task from needs_resolution back to in_progress (after resolving conflicts)
pub fn reset_task_from_resolution(task_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        require_affected(
            conn,
            "UPDATE tasks
             SET status = 'in_progress',
                 release_commit_id = NULL,
                 release_started_at = NULL,
                 claimed_heartbeat_at = ?1,
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'needs_resolution'
               AND deleted_at IS NULL",
            &[&now as &dyn rusqlite::ToSql, &task_id],
            TasksError::InvalidStatus(format!("Task {} not in needs_resolution status", task_id)),
        )
    })
}
