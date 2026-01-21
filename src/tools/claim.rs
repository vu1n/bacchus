//! Claim task tool - claims a specific task by ID, creates jj workspace
//!
//! Unlike `next`, this claims a specific task rather than the next ready one.
//! By default, only claims ready tasks (open, no blockers). Use --force to override.

use crate::db::with_db;
use crate::tasks::{self, SqliteTaskStatus};
use crate::tools::eval::{self, EventType};
use crate::workspace;
use rusqlite::params;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimOutput {
    pub success: bool,
    pub task_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub workspace_path: Option<String>,
    pub message: String,
}

/// Helper to convert TasksError to rusqlite::Error
fn tasks_error_to_rusqlite(e: tasks::TasksError) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(1),
        Some(e.to_string()),
    )
}

pub fn claim_task(task_id: &str, agent_id: &str, force: bool, workspace_root: &Path) -> Result<ClaimOutput> {
    // Use the atomic SQLite claim function
    let claim_result: std::result::Result<tasks::SqliteTask, tasks::TasksError> = if force {
        // Force claim bypasses readiness check - manually update status
        // First get the task to verify it exists
        let task = tasks::get_sqlite_task(task_id).map_err(tasks_error_to_rusqlite)?;

        if task.status == SqliteTaskStatus::Closed {
            return Ok(ClaimOutput {
                success: false,
                task_id: task_id.to_string(),
                title: Some(task.title),
                description: task.description,
                workspace_path: None,
                message: format!("Task {} is already closed", task_id),
            });
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
                             updated_at = ?2
                         WHERE id = ?3 AND deleted_at IS NULL",
                        params![agent_id, now, task_id],
                    )
                })?;

                if updated == 0 {
                    return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(format!("Task not found: {}", task_id)),
                    ));
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
            let ws = workspace::create_workspace(workspace_root, task_id).map_err(|e| {
                // Rollback: release the SQLite claim
                let _ = tasks::release_sqlite_task(task_id, agent_id);
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(format!("Failed to create workspace: {}", e)),
                )
            })?;

            // Record eval event (rework if previously completed)
            let event_type = if eval::was_previously_completed(task_id) {
                EventType::Rework
            } else {
                EventType::Started
            };
            let _ = eval::record_event(task_id, agent_id, event_type, None);

            Ok(ClaimOutput {
                success: true,
                task_id: task.id,
                title: Some(task.title),
                description: task.description,
                workspace_path: Some(ws.path.to_string_lossy().to_string()),
                message: format!("Claimed {} - work in {}", task_id, ws.path.display()),
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
        Err(e) => Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to claim task: {}", e)),
        )),
    }
}
