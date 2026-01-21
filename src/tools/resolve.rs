//! Resolve tool - mark task ready for release after resolving conflicts
//!
//! In jj workflow, after manually resolving conflicts in the workspace,
//! this validates the workspace and marks the task ready for release again.

use crate::tasks::{self, SqliteTaskStatus};
use crate::workspace;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveOutput {
    pub success: bool,
    pub task_id: String,
    pub ready_for_release: bool,
    pub commit_id: Option<String>,
    pub message: String,
}

pub fn resolve_merge(
    task_id: &str,
    workspace_root: &Path,
) -> Result<ResolveOutput, Box<dyn std::error::Error>> {
    // 1. Check task exists and is claimed
    let task = match tasks::get_sqlite_task(task_id) {
        Ok(t) => t,
        Err(tasks::TasksError::TaskNotFound(_)) => {
            return Ok(ResolveOutput {
                success: false,
                task_id: task_id.to_string(),
                ready_for_release: false,
                commit_id: None,
                message: format!("Task {} not found", task_id),
            });
        }
        Err(e) => return Err(Box::new(e)),
    };

    if task.claimed_by.is_none() {
        return Ok(ResolveOutput {
            success: false,
            task_id: task_id.to_string(),
            ready_for_release: false,
            commit_id: None,
            message: format!("No claim found for {}", task_id),
        });
    }

    let agent_id = task.claimed_by.clone().unwrap_or_default();

    // 2. Check task is in needs_resolution status (or in_progress if continuing work)
    if task.status != SqliteTaskStatus::NeedsResolution && task.status != SqliteTaskStatus::InProgress {
        return Ok(ResolveOutput {
            success: false,
            task_id: task_id.to_string(),
            ready_for_release: false,
            commit_id: None,
            message: format!(
                "Task {} is in '{}' status. Use 'bacchus release --status done' for normal completion.",
                task_id,
                task.status.as_str()
            ),
        });
    }

    // 3. Check for remaining conflicts in workspace
    if workspace::has_conflicts(workspace_root, task_id).unwrap_or(false) {
        let files = workspace::get_conflict_files(workspace_root, task_id).unwrap_or_default();
        return Ok(ResolveOutput {
            success: false,
            task_id: task_id.to_string(),
            ready_for_release: false,
            commit_id: None,
            message: format!(
                "Unresolved conflicts remain in: {}. Use 'jj resolve' to fix them.",
                files.join(", ")
            ),
        });
    }

    // 4. Validate single-commit workflow
    let commit_id = match workspace::validate_single_commit(workspace_root, task_id) {
        Ok(id) => id,
        Err(workspace::WorkspaceError::NoCommits(..)) => {
            return Ok(ResolveOutput {
                success: false,
                task_id: task_id.to_string(),
                ready_for_release: false,
                commit_id: None,
                message: format!("Task {} has no commits. Make changes before resolving.", task_id),
            });
        }
        Err(workspace::WorkspaceError::MultipleCommits(_, count)) => {
            return Ok(ResolveOutput {
                success: false,
                task_id: task_id.to_string(),
                ready_for_release: false,
                commit_id: None,
                message: format!(
                    "Task {} has {} commits. Squash to single commit before resolving.",
                    task_id, count
                ),
            });
        }
        Err(e) => {
            return Ok(ResolveOutput {
                success: false,
                task_id: task_id.to_string(),
                ready_for_release: false,
                commit_id: None,
                message: format!("Failed to validate workspace: {}", e),
            });
        }
    };

    // 5. Mark task ready for release again
    tasks::mark_task_ready_for_release(task_id, &agent_id, &commit_id)?;

    Ok(ResolveOutput {
        success: true,
        task_id: task_id.to_string(),
        ready_for_release: true,
        commit_id: Some(commit_id),
        message: format!(
            "Conflicts resolved for {}. Task marked ready for release. Orchestrator will merge.",
            task_id
        ),
    })
}
