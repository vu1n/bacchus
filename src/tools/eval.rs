//! Eval metrics tool - tracks and reports on task completion metrics
//!
//! Records events:
//! - started: agent claimed a task
//! - completed: task released with status=done
//! - failed: task released with status=failed
//! - blocked: task released with status=blocked
//! - rework: task re-claimed after being released
//! - reviewed: review command run on task

use crate::db::{with_db, with_db_str};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Event types for tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Started,
    Completed,
    Failed,
    Blocked,
    Rework,
    Reviewed,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventType::Started => "started",
            EventType::Completed => "completed",
            EventType::Failed => "failed",
            EventType::Blocked => "blocked",
            EventType::Rework => "rework",
            EventType::Reviewed => "reviewed",
        }
    }
}

/// A recorded metric event
#[derive(Debug, Serialize, Deserialize)]
pub struct MetricEvent {
    pub id: i64,
    pub task_id: String,
    pub agent_id: String,
    pub event_type: String,
    pub event_data: Option<String>,
    pub created_at: i64,
}

/// Eval summary output
#[derive(Debug, Serialize, Deserialize)]
pub struct EvalOutput {
    pub period_days: i64,
    pub total_tasks: i64,
    pub completed_tasks: i64,
    pub failed_tasks: i64,
    pub blocked_tasks: i64,
    pub rework_count: i64,
    pub completion_rate: f64,
    pub rework_rate: f64,
    pub worker_reliability: WorkerReliabilityMetrics,
    pub agent_stats: Vec<AgentStat>,
    pub recent_events: Vec<MetricEvent>,
}

/// Worker reliability metrics derived from orchestration events.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WorkerReliabilityMetrics {
    pub stale_recovered: i64,
    pub stale_recovered_heartbeat: i64,
    pub stale_recovered_runtime: i64,
    pub stale_recovered_pid_dead: i64,
    pub stale_recovered_state_mismatch: i64,
    pub kill_attempted: i64,
    pub kill_succeeded: i64,
    pub failed_worker_task_reopened: i64,
    pub worker_exit_ignored: i64,
    pub fenced_worker_exits: i64,
}

/// Per-agent statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentStat {
    pub agent_id: String,
    pub started: i64,
    pub completed: i64,
    pub failed: i64,
    pub blocked: i64,
    pub completion_rate: f64,
}

/// Record a metric event
pub fn record_event(
    task_id: &str,
    agent_id: &str,
    event_type: EventType,
    event_data: Option<&str>,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_str(|conn| {
        conn.execute(
            "INSERT INTO task_eval_metrics (task_id, agent_id, event_type, event_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, agent_id, event_type.as_str(), event_data, now],
        )?;
        Ok(())
    })
}

/// Generate eval report
pub fn generate_eval_report(epic_id: Option<&str>, days: i64) -> Result<EvalOutput, String> {
    let cutoff = chrono::Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);

    with_db_str(|conn| {
        // Build query with optional epic filter
        let epic_filter = if epic_id.is_some() {
            "AND m.task_id IN (SELECT id FROM tasks WHERE epic_id = ?2)"
        } else {
            ""
        };

        // Total unique tasks
        let total_tasks: i64 = {
            let sql = format!(
                "SELECT COUNT(DISTINCT task_id) FROM task_eval_metrics m WHERE created_at >= ?1 {}",
                epic_filter
            );
            if let Some(eid) = epic_id {
                conn.query_row(&sql, params![cutoff, eid], |r| r.get(0))?
            } else {
                conn.query_row(&sql, params![cutoff], |r| r.get(0))?
            }
        };

        // Event counts
        let count_event = |event: &str| -> i64 {
            let sql = format!(
                "SELECT COUNT(*) FROM task_eval_metrics m WHERE event_type = ?1 AND created_at >= ?2 {}",
                epic_filter
            );
            if let Some(eid) = epic_id {
                conn.query_row(&sql, params![event, cutoff, eid], |r| r.get(0))
                    .unwrap_or(0)
            } else {
                conn.query_row(&sql, params![event, cutoff], |r| r.get(0))
                    .unwrap_or(0)
            }
        };

        let completed_tasks = count_event("completed");
        let failed_tasks = count_event("failed");
        let blocked_tasks = count_event("blocked");
        let rework_count = count_event("rework");
        let started_count = count_event("started");

        // Calculate rates
        let completion_rate = if started_count > 0 {
            (completed_tasks as f64 / started_count as f64) * 100.0
        } else {
            0.0
        };

        let rework_rate = if completed_tasks > 0 {
            (rework_count as f64 / completed_tasks as f64) * 100.0
        } else {
            0.0
        };

        // Per-agent stats
        let agent_stats: Vec<AgentStat> = {
            let sql = format!(
                "SELECT agent_id,
                        SUM(CASE WHEN event_type = 'started' THEN 1 ELSE 0 END) as started,
                        SUM(CASE WHEN event_type = 'completed' THEN 1 ELSE 0 END) as completed,
                        SUM(CASE WHEN event_type = 'failed' THEN 1 ELSE 0 END) as failed,
                        SUM(CASE WHEN event_type = 'blocked' THEN 1 ELSE 0 END) as blocked
                 FROM task_eval_metrics m
                 WHERE created_at >= ?1 {}
                 GROUP BY agent_id
                 ORDER BY started DESC",
                epic_filter
            );

            let mut stmt = conn.prepare(&sql)?;
            let map_row = |row: &rusqlite::Row| -> rusqlite::Result<AgentStat> {
                let started: i64 = row.get(1)?;
                let completed: i64 = row.get(2)?;
                Ok(AgentStat {
                    agent_id: row.get(0)?,
                    started,
                    completed,
                    failed: row.get(3)?,
                    blocked: row.get(4)?,
                    completion_rate: if started > 0 {
                        (completed as f64 / started as f64) * 100.0
                    } else {
                        0.0
                    },
                })
            };

            if let Some(eid) = epic_id {
                stmt.query_map(params![cutoff, eid], map_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                stmt.query_map(params![cutoff], map_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };

        // Recent events
        let recent_events: Vec<MetricEvent> = {
            let sql = format!(
                "SELECT id, task_id, agent_id, event_type, event_data, created_at
                 FROM task_eval_metrics m
                 WHERE created_at >= ?1 {}
                 ORDER BY created_at DESC
                 LIMIT 50",
                epic_filter
            );

            let mut stmt = conn.prepare(&sql)?;
            let map_row = |row: &rusqlite::Row| -> rusqlite::Result<MetricEvent> {
                Ok(MetricEvent {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    event_type: row.get(3)?,
                    event_data: row.get(4)?,
                    created_at: row.get(5)?,
                })
            };

            if let Some(eid) = epic_id {
                stmt.query_map(params![cutoff, eid], map_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                stmt.query_map(params![cutoff], map_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };

        let worker_reliability = {
            let epic_filter = if epic_id.is_some() {
                "AND oe.entity_id IN (SELECT id FROM tasks WHERE epic_id = ?2)"
            } else {
                ""
            };
            let sql = format!(
                "SELECT oe.event_type, oe.payload
                 FROM orchestration_events oe
                 WHERE oe.created_at >= ?1
                   AND oe.entity_type = 'task'
                   AND oe.event_type IN (
                       'worker_stale_recovered',
                       'failed_worker_task_reopened',
                       'worker_exit_ignored',
                       'worker_exited'
                   )
                   {}
                 ORDER BY oe.created_at DESC",
                epic_filter
            );

            let mut stmt = conn.prepare(&sql)?;
            let rows: Vec<(String, String)> = if let Some(eid) = epic_id {
                stmt.query_map(params![cutoff, eid], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                stmt.query_map(params![cutoff], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };

            let mut metrics = WorkerReliabilityMetrics::default();
            for (event_type, payload_str) in rows {
                let payload: Value = serde_json::from_str(&payload_str).unwrap_or(Value::Null);
                match event_type.as_str() {
                    "worker_stale_recovered" => {
                        metrics.stale_recovered += 1;
                        if payload["stale_heartbeat"].as_bool().unwrap_or(false) {
                            metrics.stale_recovered_heartbeat += 1;
                        }
                        if payload["runtime_exceeded"].as_bool().unwrap_or(false) {
                            metrics.stale_recovered_runtime += 1;
                        }
                        if payload["pid_dead"].as_bool().unwrap_or(false) {
                            metrics.stale_recovered_pid_dead += 1;
                        }
                        if payload["stale_state"].as_bool().unwrap_or(false) {
                            metrics.stale_recovered_state_mismatch += 1;
                        }
                        if payload["kill_attempted"].as_bool().unwrap_or(false) {
                            metrics.kill_attempted += 1;
                        }
                        if payload["kill_succeeded"].as_bool().unwrap_or(false) {
                            metrics.kill_succeeded += 1;
                        }
                    }
                    "failed_worker_task_reopened" => {
                        metrics.failed_worker_task_reopened += 1;
                    }
                    "worker_exit_ignored" => {
                        metrics.worker_exit_ignored += 1;
                    }
                    "worker_exited" if payload["fenced"].as_bool().unwrap_or(false) => {
                        metrics.fenced_worker_exits += 1;
                    }
                    _ => {}
                }
            }
            metrics
        };

        Ok(EvalOutput {
            period_days: days,
            total_tasks,
            completed_tasks,
            failed_tasks,
            blocked_tasks,
            rework_count,
            completion_rate,
            rework_rate,
            worker_reliability,
            agent_stats,
            recent_events,
        })
    })
}

/// Check if a task was previously completed (for rework detection)
pub fn was_previously_completed(task_id: &str) -> bool {
    with_db(|conn| {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM task_eval_metrics
             WHERE task_id = ?1 AND event_type IN ('completed', 'failed')",
            [task_id],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::close_db;
    use crate::testutil::setup_test_db;

    #[test]
    fn test_eval_includes_worker_reliability_metrics() {
        let _dir = setup_test_db("EV-EPIC", "EV-001");
        let now = chrono::Utc::now().timestamp_millis();

        with_db(|conn| {
            conn.execute(
                "INSERT INTO orchestration_events (run_id, actor, event_type, entity_type, entity_id, payload, created_at)
                 VALUES (?1, 'orchestrator', 'worker_stale_recovered', 'task', 'EV-001', ?2, ?3)",
                params![
                    "run-a",
                    r#"{"stale_heartbeat":true,"runtime_exceeded":true,"pid_dead":false,"stale_state":false,"kill_attempted":true,"kill_succeeded":true}"#,
                    now
                ],
            )?;
            conn.execute(
                "INSERT INTO orchestration_events (run_id, actor, event_type, entity_type, entity_id, payload, created_at)
                 VALUES (?1, 'orchestrator', 'worker_stale_recovered', 'task', 'EV-001', ?2, ?3)",
                params![
                    "run-a",
                    r#"{"stale_heartbeat":false,"runtime_exceeded":false,"pid_dead":true,"stale_state":true,"kill_attempted":false,"kill_succeeded":false}"#,
                    now
                ],
            )?;
            conn.execute(
                "INSERT INTO orchestration_events (run_id, actor, event_type, entity_type, entity_id, payload, created_at)
                 VALUES (?1, 'orchestrator', 'failed_worker_task_reopened', 'task', 'EV-001', '{}', ?2)",
                params!["run-a", now],
            )?;
            conn.execute(
                "INSERT INTO orchestration_events (run_id, actor, event_type, entity_type, entity_id, payload, created_at)
                 VALUES (?1, 'worker', 'worker_exit_ignored', 'task', 'EV-001', '{}', ?2)",
                params!["run-a", now],
            )?;
            conn.execute(
                "INSERT INTO orchestration_events (run_id, actor, event_type, entity_type, entity_id, payload, created_at)
                 VALUES (?1, 'worker', 'worker_exited', 'task', 'EV-001', ?2, ?3)",
                params!["run-a", r#"{"fenced":true}"#, now],
            )?;
            Ok(())
        })
        .unwrap();

        let report = generate_eval_report(None, 7).unwrap();
        assert_eq!(report.worker_reliability.stale_recovered, 2);
        assert_eq!(report.worker_reliability.stale_recovered_heartbeat, 1);
        assert_eq!(report.worker_reliability.stale_recovered_runtime, 1);
        assert_eq!(report.worker_reliability.stale_recovered_pid_dead, 1);
        assert_eq!(report.worker_reliability.stale_recovered_state_mismatch, 1);
        assert_eq!(report.worker_reliability.kill_attempted, 1);
        assert_eq!(report.worker_reliability.kill_succeeded, 1);
        assert_eq!(report.worker_reliability.failed_worker_task_reopened, 1);
        assert_eq!(report.worker_reliability.worker_exit_ignored, 1);
        assert_eq!(report.worker_reliability.fenced_worker_exits, 1);

        close_db();
    }
}
