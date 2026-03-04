//! Claim task tool - claims a specific task by ID, creates jj workspace
//!
//! Unlike `next`, this claims a specific task rather than the next ready one.
//! By default, only claims ready tasks (open, no blockers). Use --force to override.

use crate::db::with_db;
use crate::tasks::{self, SqliteTaskStatus};
use crate::tools::eval::{self, EventType};
use crate::tools::session;
use crate::workspace;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::ToolError;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimOutput {
    pub success: bool,
    pub task_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub workspace_path: Option<String>,
    pub message: String,
}

pub fn claim_task(
    task_id: &str,
    agent_id: &str,
    force: bool,
    workspace_root: &Path,
) -> Result<ClaimOutput, ToolError> {
    let mut rollback_snapshot: Option<tasks::SqliteTask> = None;

    // Use the atomic SQLite claim function
    let claim_result: std::result::Result<tasks::SqliteTask, tasks::TasksError> = if force {
        // Force claim bypasses readiness check - manually update status
        // First get the task to verify it exists
        let task = tasks::get_sqlite_task(task_id)?;
        rollback_snapshot = Some(task.clone());

        match task.status {
            SqliteTaskStatus::Open | SqliteTaskStatus::InProgress | SqliteTaskStatus::Blocked => {}
            _ => {
                return Ok(ClaimOutput {
                    success: false,
                    task_id: task_id.to_string(),
                    title: Some(task.title),
                    description: task.description,
                    workspace_path: None,
                    message: format!(
                        "Task {} is in '{}' status and cannot be force-claimed. Use resolve/abort/release flow.",
                        task_id,
                        task.status.as_str()
                    ),
                });
            }
        }

        // For force claim, we need to directly claim without readiness check
        // Use the regular claim but catch the "not ready" error and force through
        match tasks::claim_sqlite_task(task_id, agent_id) {
            Ok(t) => Ok(t),
            Err(tasks::TasksError::NotReady(_)) => {
                let now = chrono::Utc::now().timestamp_millis();
                let updated = with_db(|conn| {
                    conn.execute(
                        "UPDATE tasks
                         SET status = 'in_progress',
                             claimed_by = ?1,
                             claimed_at = ?2,
                             claimed_heartbeat_at = ?2,
                             updated_at = ?2
                         WHERE id = ?3
                           AND status IN ('open', 'in_progress', 'blocked')
                           AND deleted_at IS NULL",
                        params![agent_id, now, task_id],
                    )
                })?;

                if updated == 0 {
                    return Err(tasks::TasksError::TaskNotFound(task_id.to_string()).into());
                }
                // Now get the updated task
                tasks::get_sqlite_task(task_id)
            }
            Err(e) => Err(e),
        }
    } else {
        tasks::claim_sqlite_task(task_id, agent_id)
    };

    match claim_result {
        Ok(task) => {
            // Create jj workspace for the task
            let ws = match workspace::create_workspace(workspace_root, task_id) {
                Ok(ws) => ws,
                Err(e) => {
                    // Rollback: preserve prior state when force-claiming.
                    if let Some(prev) = rollback_snapshot {
                        let now = chrono::Utc::now().timestamp_millis();
                        let _ = with_db(|conn| {
                            conn.execute(
                                "UPDATE tasks
                                 SET status = ?1,
                                     claimed_by = ?2,
                                     claimed_at = ?3,
                                     claimed_heartbeat_at = ?4,
                                     updated_at = ?5
                                 WHERE id = ?6
                                   AND deleted_at IS NULL",
                                params![
                                    prev.status.as_str(),
                                    prev.claimed_by,
                                    prev.claimed_at,
                                    prev.claimed_heartbeat_at,
                                    now,
                                    task_id
                                ],
                            )
                        });
                    } else {
                        let _ = tasks::reset_sqlite_task(task_id, SqliteTaskStatus::Open);
                    }
                    return Err(format!("Failed to create workspace: {}", e).into());
                }
            };

            // Record eval event (rework if previously completed)
            let event_type = if eval::was_previously_completed(task_id) {
                EventType::Rework
            } else {
                EventType::Started
            };
            let _ = eval::record_event(task_id, agent_id, event_type, None);
            let heartbeat_warning =
                session::attach_agent_session_heartbeat(task_id, agent_id).err();

            let mut message = format!("Claimed {} - work in {}", task_id, ws.path.display());
            if let Some(warn) = heartbeat_warning {
                message = format!("{} (heartbeat loop unavailable: {})", message, warn);
            }

            Ok(ClaimOutput {
                success: true,
                task_id: task.id,
                title: Some(task.title),
                description: task.description,
                workspace_path: Some(ws.path.to_string_lossy().to_string()),
                message,
            })
        }
        Err(tasks::TasksError::NotReady(msg)) => {
            let task = tasks::get_sqlite_task(task_id).ok();
            Ok(ClaimOutput {
                success: false,
                task_id: task_id.to_string(),
                title: task.as_ref().map(|t| t.title.clone()),
                description: task.and_then(|t| t.description),
                workspace_path: None,
                message: format!(
                    "Task {} is not ready: {}. Use --force to override.",
                    task_id, msg
                ),
            })
        }
        Err(tasks::TasksError::TaskNotFound(_)) => Ok(ClaimOutput {
            success: false,
            task_id: task_id.to_string(),
            title: None,
            description: None,
            workspace_path: None,
            message: format!("Task {} not found", task_id),
        }),
        Err(e) => Err(e.into()),
    }
}
