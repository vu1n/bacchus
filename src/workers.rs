//! Worker lifecycle tracking for orchestrator-managed agent processes.
//!
//! Tracks launch attempts, active workers, failures, and completion.

use crate::db::with_db_str;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRetryState {
    pub attempts: i32,
    pub last_failed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveWorkerSnapshot {
    pub worker_id: i64,
    pub task_id: String,
    pub agent_id: String,
    pub status: String,
    pub attempt: i32,
    pub pid: Option<i64>,
    pub started_at: i64,
    pub updated_at: i64,
    pub task_status: Option<String>,
    pub task_claimed_by: Option<String>,
    pub task_last_seen_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRunStats {
    pub launching: i64,
    pub running: i64,
    pub completed: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedWorkerTaskCandidate {
    pub worker_id: i64,
    pub task_id: String,
    pub agent_id: String,
}

/// Reserve a worker launch attempt row.
pub fn create_worker_attempt(
    run_id: &str,
    task_id: &str,
    agent_id: &str,
    scope_id: &str,
    command: &str,
    attempt: i32,
) -> Result<i64, String> {
    let now = chrono::Utc::now().timestamp_millis();
    with_db_str(|conn| {
        conn.execute(
            "INSERT INTO agent_workers
             (run_id, task_id, agent_id, scope_id, command, status, attempt, started_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'launching', ?6, ?7, ?7)",
            params![run_id, task_id, agent_id, scope_id, command, attempt, now],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn mark_worker_running(worker_id: i64, pid: Option<i64>) -> Result<bool, String> {
    let now = chrono::Utc::now().timestamp_millis();
    with_db_str(|conn| {
        let updated = conn.execute(
            "UPDATE agent_workers
             SET status = 'running', pid = ?1, updated_at = ?2, error = NULL
             WHERE id = ?3 AND status = 'launching'",
            params![pid, now, worker_id],
        )?;
        Ok(updated > 0)
    })
}

pub fn mark_worker_completed(worker_id: i64, exit_code: Option<i32>) -> Result<bool, String> {
    let now = chrono::Utc::now().timestamp_millis();
    with_db_str(|conn| {
        let updated = conn.execute(
            "UPDATE agent_workers
             SET status = 'completed', exit_code = ?1, updated_at = ?2, error = NULL
             WHERE id = ?3 AND status IN ('launching', 'running')",
            params![exit_code, now, worker_id],
        )?;
        Ok(updated > 0)
    })
}

pub fn mark_worker_failed(
    worker_id: i64,
    error: &str,
    exit_code: Option<i32>,
) -> Result<bool, String> {
    let now = chrono::Utc::now().timestamp_millis();
    with_db_str(|conn| {
        let updated = conn.execute(
            "UPDATE agent_workers
             SET status = 'failed', error = ?1, exit_code = ?2, updated_at = ?3
             WHERE id = ?4 AND status IN ('launching', 'running')",
            params![error, exit_code, now, worker_id],
        )?;
        Ok(updated > 0)
    })
}

pub fn get_retry_state(run_id: &str, task_id: &str) -> Result<WorkerRetryState, String> {
    with_db_str(|conn| {
        let attempts: i32 = conn
            .query_row(
                "SELECT COALESCE(MAX(attempt), 0)
                 FROM agent_workers
                 WHERE run_id = ?1 AND task_id = ?2",
                params![run_id, task_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let last_failed_at: Option<i64> = conn
            .query_row(
                "SELECT updated_at
                 FROM agent_workers
                 WHERE run_id = ?1 AND task_id = ?2 AND status = 'failed'
                 ORDER BY updated_at DESC
                 LIMIT 1",
                params![run_id, task_id],
                |row| row.get(0),
            )
            .ok();

        Ok(WorkerRetryState {
            attempts,
            last_failed_at,
        })
    })
}

pub fn count_active_workers(run_id: &str) -> Result<usize, String> {
    with_db_str(|conn| {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_workers
                 WHERE run_id = ?1 AND status IN ('launching', 'running')",
                [run_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count.max(0) as usize)
    })
}

pub fn fail_active_workers(run_id: &str, reason: &str) -> Result<usize, String> {
    let now = chrono::Utc::now().timestamp_millis();
    with_db_str(|conn| {
        let updated = conn.execute(
            "UPDATE agent_workers
             SET status = 'failed', error = ?1, updated_at = ?2
             WHERE run_id = ?3 AND status IN ('launching', 'running')",
            params![reason, now, run_id],
        )?;
        Ok(updated)
    })
}

pub fn list_active_worker_snapshots(run_id: &str) -> Result<Vec<ActiveWorkerSnapshot>, String> {
    with_db_str(|conn| {
        let mut stmt = conn.prepare(
            "SELECT aw.id, aw.task_id, aw.agent_id, aw.status, aw.attempt, aw.pid, aw.started_at, aw.updated_at,
                    t.status, t.claimed_by, COALESCE(t.claimed_heartbeat_at, t.claimed_at)
             FROM agent_workers aw
             LEFT JOIN tasks t ON t.id = aw.task_id
             WHERE aw.run_id = ?1
               AND aw.status IN ('launching', 'running')
             ORDER BY aw.id",
        )?;
        let rows = stmt
            .query_map([run_id], |row| {
                Ok(ActiveWorkerSnapshot {
                    worker_id: row.get(0)?,
                    task_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    status: row.get(3)?,
                    attempt: row.get(4)?,
                    pid: row.get(5)?,
                    started_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    task_status: row.get(8)?,
                    task_claimed_by: row.get(9)?,
                    task_last_seen_at: row.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

pub fn get_run_worker_stats(run_id: &str) -> Result<WorkerRunStats, String> {
    with_db_str(|conn| {
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*)
             FROM agent_workers
             WHERE run_id = ?1
             GROUP BY status",
        )?;
        let rows = stmt
            .query_map([run_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut stats = WorkerRunStats {
            launching: 0,
            running: 0,
            completed: 0,
            failed: 0,
        };

        for (status, count) in rows {
            match status.as_str() {
                "launching" => stats.launching = count,
                "running" => stats.running = count,
                "completed" => stats.completed = count,
                "failed" => stats.failed = count,
                _ => {}
            }
        }

        Ok(stats)
    })
}

pub fn list_reopenable_failed_worker_tasks(
    run_id: &str,
) -> Result<Vec<FailedWorkerTaskCandidate>, String> {
    with_db_str(|conn| {
        let mut stmt = conn.prepare(
            "SELECT aw.id, aw.task_id, aw.agent_id
             FROM agent_workers aw
             JOIN tasks t ON t.id = aw.task_id
             WHERE aw.run_id = ?1
               AND aw.status = 'failed'
               AND t.status = 'in_progress'
               AND t.claimed_by = aw.agent_id
               AND t.deleted_at IS NULL
             ORDER BY aw.id",
        )?;
        let rows = stmt
            .query_map([run_id], |row| {
                Ok(FailedWorkerTaskCandidate {
                    worker_id: row.get(0)?,
                    task_id: row.get(1)?,
                    agent_id: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

pub fn get_worker_status(worker_id: i64) -> Result<Option<String>, String> {
    with_db_str(|conn| {
        let status = conn
            .query_row(
                "SELECT status FROM agent_workers WHERE id = ?1",
                [worker_id],
                |row| row.get::<_, String>(0),
            )
            .ok();
        Ok(status)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::close_db;
    use crate::testutil::setup_test_db;

    fn setup() -> tempfile::TempDir {
        setup_test_db("W-EPIC", "W-001")
    }

    #[test]
    fn test_worker_attempt_lifecycle() {
        let _dir = setup();

        let worker_id =
            create_worker_attempt("run-1", "W-001", "agent-a", "scope-a", "echo hi", 1).unwrap();
        assert!(mark_worker_running(worker_id, Some(1234)).unwrap());
        assert_eq!(count_active_workers("run-1").unwrap(), 1);

        assert!(mark_worker_completed(worker_id, Some(0)).unwrap());
        assert_eq!(count_active_workers("run-1").unwrap(), 0);

        let retry = get_retry_state("run-1", "W-001").unwrap();
        assert_eq!(retry.attempts, 1);

        close_db();
    }

    #[test]
    fn test_fail_active_workers() {
        let _dir = setup();

        let worker_id =
            create_worker_attempt("run-2", "W-001", "agent-a", "scope-a", "echo hi", 1).unwrap();
        assert!(mark_worker_running(worker_id, Some(1234)).unwrap());

        let failed = fail_active_workers("run-2", "stop").unwrap();
        assert_eq!(failed, 1);
        assert_eq!(count_active_workers("run-2").unwrap(), 0);

        close_db();
    }

    #[test]
    fn test_active_worker_snapshots_and_stats() {
        let _dir = setup();
        let now = chrono::Utc::now().timestamp_millis();

        with_db_str(|conn| {
            conn.execute(
                "UPDATE tasks
                 SET status = 'in_progress',
                     claimed_by = 'agent-z',
                     claimed_at = ?1,
                     claimed_heartbeat_at = ?1
                 WHERE id = 'W-001'",
                [now],
            )?;
            Ok(())
        })
        .unwrap();

        let worker_id =
            create_worker_attempt("run-3", "W-001", "agent-z", "scope-z", "echo hi", 1).unwrap();
        assert!(mark_worker_running(worker_id, Some(4242)).unwrap());

        let snapshots = list_active_worker_snapshots("run-3").unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].task_status.as_deref(), Some("in_progress"));
        assert_eq!(snapshots[0].task_claimed_by.as_deref(), Some("agent-z"));

        assert!(mark_worker_completed(worker_id, Some(0)).unwrap());
        let stats = get_run_worker_stats("run-3").unwrap();
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.running, 0);

        close_db();
    }

    #[test]
    fn test_list_reopenable_failed_worker_tasks() {
        let _dir = setup();
        let now = chrono::Utc::now().timestamp_millis();

        with_db_str(|conn| {
            conn.execute(
                "UPDATE tasks
                 SET status = 'in_progress',
                     claimed_by = 'agent-r',
                     claimed_at = ?1,
                     claimed_heartbeat_at = ?1
                 WHERE id = 'W-001'",
                [now],
            )?;
            Ok(())
        })
        .unwrap();

        let worker_id =
            create_worker_attempt("run-4", "W-001", "agent-r", "scope-r", "echo hi", 1).unwrap();
        assert!(mark_worker_failed(worker_id, "boom", Some(1)).unwrap());

        let reopenable = list_reopenable_failed_worker_tasks("run-4").unwrap();
        assert_eq!(reopenable.len(), 1);
        assert_eq!(reopenable[0].task_id, "W-001");

        close_db();
    }

    #[test]
    fn test_terminal_status_transition_is_fenced() {
        let _dir = setup();
        let worker_id =
            create_worker_attempt("run-5", "W-001", "agent-f", "scope-f", "echo hi", 1).unwrap();
        assert!(mark_worker_running(worker_id, Some(5050)).unwrap());
        assert!(mark_worker_failed(worker_id, "failed once", Some(1)).unwrap());
        assert!(!mark_worker_completed(worker_id, Some(0)).unwrap());
        assert_eq!(
            get_worker_status(worker_id).unwrap().as_deref(),
            Some("failed")
        );
        close_db();
    }
}
