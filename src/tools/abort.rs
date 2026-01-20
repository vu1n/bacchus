//! Abort tool - abort a failed merge for a task
//!
//! Restores the repository to pre-merge state when a merge conflict occurs.
//! Uses SQLite-based task management.

use crate::tasks::{self, TasksError};
use crate::worktree;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct AbortOutput {
    pub success: bool,
    pub task_id: String,
    pub message: String,
}

pub fn abort_merge(
    task_id: &str,
    workspace_root: &Path,
) -> Result<AbortOutput, Box<dyn std::error::Error>> {
    // 1. Check task exists and is claimed
    let task = match tasks::get_sqlite_task(task_id) {
        Ok(t) => t,
        Err(TasksError::TaskNotFound(_)) => {
            return Ok(AbortOutput {
                success: false,
                task_id: task_id.to_string(),
                message: format!("Task {} not found", task_id),
            });
        }
        Err(e) => return Err(Box::new(e)),
    };

    if task.claimed_by.is_none() {
        return Ok(AbortOutput {
            success: false,
            task_id: task_id.to_string(),
            message: format!("No claim found for {}", task_id),
        });
    }

    // 2. Check we're in a merge conflict state
    if !worktree::is_in_merge_conflict(workspace_root)? {
        return Ok(AbortOutput {
            success: false,
            task_id: task_id.to_string(),
            message: "Not in a merge conflict state. Nothing to abort.".to_string(),
        });
    }

    // 3. Verify the merge is for this task's branch
    let merge_branch = worktree::get_merge_branch(workspace_root)?;
    let expected = format!("bacchus/{}", task_id);

    if let Some(ref branch) = merge_branch {
        if branch != &expected {
            return Ok(AbortOutput {
                success: false,
                task_id: task_id.to_string(),
                message: format!(
                    "Current merge conflict is for '{}', not '{}'. Abort the correct task.",
                    branch, expected
                ),
            });
        }
    }

    // 4. Abort the merge
    worktree::abort_merge(workspace_root)?;

    Ok(AbortOutput {
        success: true,
        task_id: task_id.to_string(),
        message: format!(
            "Aborted merge for {}. Worktree preserved at .bacchus/worktrees/{}. Continue working or release with --status failed.",
            task_id, task_id
        ),
    })
}
