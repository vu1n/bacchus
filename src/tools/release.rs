//! Release task tool - merges or discards worktree, updates task status
//!
//! Handles completing, blocking, or failing a claimed task.
//! Uses SQLite-based task management (tasks_v2 table).

use crate::tasks::{self, SqliteTaskStatus};
use crate::worktree;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseOutput {
    pub success: bool,
    pub task_id: String,
    pub status: String,
    pub merged: bool,
    pub message: String,
}

pub fn release_bead(
    task_id: &str,
    status: &str,
    workspace_root: &Path,
) -> Result<ReleaseOutput, Box<dyn std::error::Error>> {
    // Check if SQLite task has a claimed_by
    let task = match tasks::get_sqlite_task(task_id) {
        Ok(t) => t,
        Err(tasks::TasksError::TaskNotFound(_)) => {
            return Ok(ReleaseOutput {
                success: false,
                task_id: task_id.to_string(),
                status: status.to_string(),
                merged: false,
                message: format!("Task {} not found", task_id),
            });
        }
        Err(e) => return Err(Box::new(e)),
    };

    if task.claimed_by.is_none() {
        return Ok(ReleaseOutput {
            success: false,
            task_id: task_id.to_string(),
            status: status.to_string(),
            merged: false,
            message: format!("No claim found for {}", task_id),
        });
    }

    let agent_id = task.claimed_by.clone().unwrap_or_default();
    let mut merged = false;

    match status {
        "done" => {
            // Merge worktree branch to main, then cleanup
            if let Err(e) = worktree::merge_worktree(workspace_root, task_id, "main") {
                let is_conflict = worktree::is_in_merge_conflict(workspace_root).unwrap_or(false);

                let message = if is_conflict {
                    format!(
                        "Merge conflict detected. Options:\n\
                         1. Resolve conflicts manually, then: bacchus resolve {}\n\
                         2. Abort merge, keep working: bacchus abort {}\n\
                         3. Discard all work: bacchus release {} --status failed",
                        task_id, task_id, task_id
                    )
                } else {
                    format!("Failed to merge: {}", e)
                };

                return Ok(ReleaseOutput {
                    success: false,
                    task_id: task_id.to_string(),
                    status: status.to_string(),
                    merged: false,
                    message,
                });
            }
            merged = true;

            // Remove worktree
            worktree::remove_worktree(workspace_root, task_id, false)?;

            // Release SQLite task (marks as closed)
            if !agent_id.is_empty() {
                tasks::release_sqlite_task(task_id, &agent_id)?;
            } else {
                // No agent_id, manually close
                tasks::update_sqlite_task_status(task_id, SqliteTaskStatus::Closed)?;
            }
        }
        "blocked" => {
            // Keep worktree, mark as blocked
            tasks::update_sqlite_task_status(task_id, SqliteTaskStatus::Blocked)?;
        }
        "failed" => {
            // Discard worktree, reset to open
            worktree::remove_worktree(workspace_root, task_id, true)?;
            tasks::update_sqlite_task_status(task_id, SqliteTaskStatus::Open)?;
        }
        _ => {
            return Ok(ReleaseOutput {
                success: false,
                task_id: task_id.to_string(),
                status: status.to_string(),
                merged: false,
                message: format!("Invalid status: {}. Use done, blocked, or failed", status),
            });
        }
    }

    Ok(ReleaseOutput {
        success: true,
        task_id: task_id.to_string(),
        status: status.to_string(),
        merged,
        message: format!("Released {} with status {}", task_id, status),
    })
}
