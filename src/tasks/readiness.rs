//! Readiness logic for task claiming and scheduling.

use rusqlite::params;

use crate::db::{with_db, with_db_typed};

use super::crud::{map_sqlite_task_row, TASK_SELECT_COLUMNS};
use super::lease::CLAIM_HEARTBEAT_TIMEOUT_MS;
use super::types::*;

/// Build the SQL WHERE predicates that define task readiness.
///
/// A task is ready when:
/// 1. All dependency tasks are closed (or deleted)
/// 2. No in-progress tasks have overlapping footprints
///
/// `task_ref` - SQL expression for the task ID (e.g. "t.id" or "?3")
/// `cutoff_ref` - SQL expression for the heartbeat staleness cutoff (e.g. "?1" or "?3")
pub(crate) fn readiness_predicates(task_ref: &str, cutoff_ref: &str) -> String {
    let overlap_join = super::queries::FOOTPRINT_OVERLAP_JOIN;
    format!(
        r#"AND NOT EXISTS (
                  SELECT 1 FROM task_dependencies td
                  JOIN tasks dep ON dep.id = td.depends_on
                  WHERE td.task_id = {task_ref}
                    AND dep.status != 'closed'
                    AND dep.deleted_at IS NULL
              )
              AND NOT EXISTS (
                  SELECT 1 FROM {overlap_join}
                  WHERE fp1.task_id = {task_ref}
                    AND other.id != {task_ref}
                    AND other.status = 'in_progress'
                    AND COALESCE(other.claimed_heartbeat_at, other.claimed_at, 0) >= {cutoff_ref}
                    AND other.deleted_at IS NULL
              )"#
    )
}

/// Claim the next ready SQLite task atomically
///
/// Readiness = open + not deleted + deps satisfied + no footprint collision
/// Returns None if no ready tasks available.
pub fn claim_next_sqlite_task(agent_id: &str) -> Result<Option<SqliteTask>, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();
    let active_cutoff = now - CLAIM_HEARTBEAT_TIMEOUT_MS;

    with_db(|conn| {
        let ready = readiness_predicates("t.id", "?3");
        let sql = format!(
            r#"
            UPDATE tasks
            SET status = 'in_progress',
                claimed_by = ?1,
                claimed_at = ?2,
                claimed_heartbeat_at = ?2,
                updated_at = ?2
            WHERE id = (
                SELECT t.id FROM tasks t
                WHERE t.status = 'open'
                  AND t.deleted_at IS NULL
                  {ready}
                ORDER BY t.priority, t.created_at
                LIMIT 1
            )
            RETURNING {}
            "#,
            TASK_SELECT_COLUMNS
        );

        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params![agent_id, now, active_cutoff])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(map_sqlite_task_row(row)?));
        }

        Ok(None)
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}

/// Claim a specific SQLite task atomically
///
/// Returns error if task is not ready (deps not satisfied, footprint collision, etc.)
pub fn claim_sqlite_task(task_id: &str, agent_id: &str) -> Result<SqliteTask, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();
    let active_cutoff = now - CLAIM_HEARTBEAT_TIMEOUT_MS;

    with_db_typed(|conn| {
        let ready = readiness_predicates("?3", "?4");
        let sql = format!(
            r#"
            UPDATE tasks
            SET status = 'in_progress',
                claimed_by = ?1,
                claimed_at = ?2,
                claimed_heartbeat_at = ?2,
                updated_at = ?2
            WHERE id = ?3
              AND status = 'open'
              AND deleted_at IS NULL
              {ready}
            "#
        );
        let affected = conn.execute(&sql, params![agent_id, now, task_id, active_cutoff])?;

        if affected == 0 {
            // Check why claim failed — return precise domain errors
            let task_status: Option<String> = conn
                .query_row(
                    "SELECT status FROM tasks WHERE id = ?1 AND deleted_at IS NULL",
                    [task_id],
                    |row| row.get(0),
                )
                .ok();

            return Err(match task_status {
                None => TasksError::TaskNotFound(task_id.to_string()),
                Some(s) if s != "open" => {
                    TasksError::NotReady(format!("Task {} has status '{}', not 'open'", task_id, s))
                }
                _ => TasksError::NotReady(format!(
                    "Task {} is not ready (deps or footprint collision)",
                    task_id
                )),
            });
        }

        // Fetch the claimed task
        Ok(conn.query_row(
            &format!("SELECT {} FROM tasks WHERE id = ?1", TASK_SELECT_COLUMNS),
            [task_id],
            map_sqlite_task_row,
        )?)
    })
}

/// Release a SQLite task (mark as closed, clear claim)
#[cfg(test)]
pub fn release_sqlite_task(task_id: &str, agent_id: &str) -> Result<SqliteTask, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        super::crud::require_affected(
            conn,
            "UPDATE tasks
             SET status = 'closed',
                 claimed_by = NULL,
                 claimed_at = NULL,
                 claimed_heartbeat_at = NULL,
                 updated_at = ?1
             WHERE id = ?2 AND claimed_by = ?3 AND status = 'in_progress'",
            &[&now as &dyn rusqlite::ToSql, &task_id, &agent_id],
            TasksError::TaskNotFound(format!(
                "Task {} not owned by {} or not in_progress",
                task_id, agent_id
            )),
        )?;

        // Fetch the released task
        Ok(conn.query_row(
            &format!("SELECT {} FROM tasks WHERE id = ?1", TASK_SELECT_COLUMNS),
            [task_id],
            map_sqlite_task_row,
        )?)
    })
}

/// Get ready SQLite tasks (for display/debugging)
pub fn get_ready_sqlite_tasks(epic_id: Option<&str>) -> Result<Vec<SqliteTask>, TasksError> {
    with_db(|conn| {
        let now = chrono::Utc::now().timestamp_millis();
        let active_cutoff = now - CLAIM_HEARTBEAT_TIMEOUT_MS;

        // Parameter layout: ?1 = active_cutoff (always), ?2 = epic_id (optional)
        let epic_filter = if epic_id.is_some() {
            "AND t.epic_id = ?2"
        } else {
            ""
        };

        let ready = readiness_predicates("t.id", "?1");
        let sql = format!(
            r#"
            SELECT t.id, t.epic_id, t.title, t.description, t.priority, t.status, t.task_type, t.archetype,
                   t.claimed_by, t.claimed_at, t.claimed_heartbeat_at, t.ready_commit_id, t.release_commit_id, t.release_started_at,
                   t.created_at, t.updated_at, t.deleted_at
            FROM tasks t
            WHERE t.status = 'open'
              AND t.deleted_at IS NULL
              {epic_filter}
              {ready}
            ORDER BY t.priority, t.created_at
        "#
        );

        let mut stmt = conn.prepare(&sql)?;

        let tasks: Vec<SqliteTask> = if let Some(eid) = epic_id {
            stmt.query_map(params![active_cutoff, eid], map_sqlite_task_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![active_cutoff], map_sqlite_task_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        Ok(tasks)
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}
