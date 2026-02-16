//! Eval metrics tool - tracks and reports on task completion metrics
//!
//! Records events:
//! - started: agent claimed a task
//! - completed: task released with status=done
//! - failed: task released with status=failed
//! - blocked: task released with status=blocked
//! - rework: task re-claimed after being released
//! - reviewed: review command run on task

use crate::db::with_db;
use rusqlite::params;
use serde::{Deserialize, Serialize};

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

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "started" => Some(EventType::Started),
            "completed" => Some(EventType::Completed),
            "failed" => Some(EventType::Failed),
            "blocked" => Some(EventType::Blocked),
            "rework" => Some(EventType::Rework),
            "reviewed" => Some(EventType::Reviewed),
            _ => None,
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
    pub agent_stats: Vec<AgentStat>,
    pub recent_events: Vec<MetricEvent>,
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

    with_db(|conn| {
        conn.execute(
            "INSERT INTO task_eval_metrics (task_id, agent_id, event_type, event_data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![task_id, agent_id, event_type.as_str(), event_data, now],
        )?;
        Ok(())
    })
    .map_err(|e: rusqlite::Error| e.to_string())
}

/// Generate eval report
pub fn generate_eval_report(epic_id: Option<&str>, days: i64) -> Result<EvalOutput, String> {
    let cutoff = chrono::Utc::now().timestamp_millis() - (days * 24 * 60 * 60 * 1000);

    with_db(|conn| {
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
                conn.query_row(&sql, params![event, cutoff, eid], |r| r.get(0)).unwrap_or(0)
            } else {
                conn.query_row(&sql, params![event, cutoff], |r| r.get(0)).unwrap_or(0)
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
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                stmt.query_map(params![cutoff], map_row)?
                    .filter_map(|r| r.ok())
                    .collect()
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
                    .filter_map(|r| r.ok())
                    .collect()
            } else {
                stmt.query_map(params![cutoff], map_row)?
                    .filter_map(|r| r.ok())
                    .collect()
            }
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
            agent_stats,
            recent_events,
        })
    })
    .map_err(|e: rusqlite::Error| e.to_string())
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
