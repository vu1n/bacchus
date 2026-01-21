//! Release task tool - marks task ready for release or discards workspace
//!
//! In jj workflow, agents don't merge directly. Instead:
//! - "done": Validates single-commit, marks task ready_for_release
//! - "blocked": Keeps workspace, marks as blocked
//! - "failed": Removes workspace, resets to open
//!
//! The orchestrator handles actual release (rebase onto main, advance bookmark).

use crate::tasks::{self, SqliteTaskStatus};
use crate::tools::eval::{self, EventType};
use crate::workspace;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseOutput {
    pub success: bool,
    pub task_id: String,
    pub status: String,
    pub ready_for_release: bool,
    pub commit_id: Option<String>,
    pub message: String,
}

pub fn release_task(
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
                ready_for_release: false,
                commit_id: None,
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
            ready_for_release: false,
            commit_id: None,
            message: format!("No claim found for {}", task_id),
        });
    }

    let agent_id = task.claimed_by.clone().unwrap_or_default();

    match status {
        "done" => {
            // Validate single-commit workflow before marking ready
            let commit_id = match workspace::validate_single_commit(workspace_root, task_id) {
                Ok(id) => id,
                Err(workspace::WorkspaceError::NoCommits(..)) => {
                    return Ok(ReleaseOutput {
                        success: false,
                        task_id: task_id.to_string(),
                        status: status.to_string(),
                        ready_for_release: false,
                        commit_id: None,
                        message: format!(
                            "Task {} has no commits. Make changes before marking done.",
                            task_id
                        ),
                    });
                }
                Err(workspace::WorkspaceError::MultipleCommits(_, count)) => {
                    return Ok(ReleaseOutput {
                        success: false,
                        task_id: task_id.to_string(),
                        status: status.to_string(),
                        ready_for_release: false,
                        commit_id: None,
                        message: format!(
                            "Task {} has {} commits. Squash to single commit before marking done.",
                            task_id, count
                        ),
                    });
                }
                Err(e) => {
                    return Ok(ReleaseOutput {
                        success: false,
                        task_id: task_id.to_string(),
                        status: status.to_string(),
                        ready_for_release: false,
                        commit_id: None,
                        message: format!("Failed to validate workspace: {}", e),
                    });
                }
            };

            // Check for conflicts before marking ready
            if workspace::has_conflicts(workspace_root, task_id).unwrap_or(false) {
                let files = workspace::get_conflict_files(workspace_root, task_id).unwrap_or_default();
                return Ok(ReleaseOutput {
                    success: false,
                    task_id: task_id.to_string(),
                    status: status.to_string(),
                    ready_for_release: false,
                    commit_id: None,
                    message: format!(
                        "Task {} has conflicts in: {}. Resolve before marking done.",
                        task_id,
                        files.join(", ")
                    ),
                });
            }

            // Mark task ready for release (orchestrator will handle actual merge)
            tasks::mark_task_ready_for_release(task_id, &agent_id, &commit_id)?;

            // Record eval event
            let _ = eval::record_event(task_id, &agent_id, EventType::Completed, None);

            Ok(ReleaseOutput {
                success: true,
                task_id: task_id.to_string(),
                status: "ready_for_release".to_string(),
                ready_for_release: true,
                commit_id: Some(commit_id),
                message: format!(
                    "Task {} marked ready for release. Orchestrator will merge.",
                    task_id
                ),
            })
        }
        "blocked" => {
            // Keep workspace, mark as blocked
            tasks::reset_sqlite_task(task_id, SqliteTaskStatus::Blocked)?;

            // Record eval event
            let _ = eval::record_event(task_id, &agent_id, EventType::Blocked, None);

            Ok(ReleaseOutput {
                success: true,
                task_id: task_id.to_string(),
                status: status.to_string(),
                ready_for_release: false,
                commit_id: None,
                message: format!("Task {} marked as blocked. Workspace preserved.", task_id),
            })
        }
        "failed" => {
            // Remove workspace, reset to open
            let _ = workspace::remove_workspace(workspace_root, task_id, true);
            tasks::reset_sqlite_task(task_id, SqliteTaskStatus::Open)?;

            // Record eval event
            let _ = eval::record_event(task_id, &agent_id, EventType::Failed, None);

            Ok(ReleaseOutput {
                success: true,
                task_id: task_id.to_string(),
                status: status.to_string(),
                ready_for_release: false,
                commit_id: None,
                message: format!("Task {} failed. Workspace removed, task reset to open.", task_id),
            })
        }
        _ => Ok(ReleaseOutput {
            success: false,
            task_id: task_id.to_string(),
            status: status.to_string(),
            ready_for_release: false,
            commit_id: None,
            message: format!("Invalid status: {}. Use done, blocked, or failed", status),
        }),
    }
}
