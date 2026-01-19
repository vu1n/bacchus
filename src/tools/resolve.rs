//! Resolve tool - complete a merge after manual conflict resolution
//!
//! Finishes the merge, removes worktree, and updates task status.
//! Uses SQLite-based task management (tasks_v2 table).

use crate::tasks::{self, SqliteTaskStatus};
use crate::worktree;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveOutput {
    pub success: bool,
    pub task_id: String,
    pub merged: bool,
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
                merged: false,
                message: format!("Task {} not found", task_id),
            });
        }
        Err(e) => return Err(Box::new(e)),
    };

    if task.claimed_by.is_none() {
        return Ok(ResolveOutput {
            success: false,
            task_id: task_id.to_string(),
            merged: false,
            message: format!("No claim found for {}", task_id),
        });
    }

    let agent_id = task.claimed_by.clone().unwrap_or_default();

    // 2. Check we're in a merge state
    if !worktree::is_in_merge_conflict(workspace_root)? {
        return Ok(ResolveOutput {
            success: false,
            task_id: task_id.to_string(),
            merged: false,
            message: "Not in a merge state. Use 'bacchus release --status done' instead.".to_string(),
        });
    }

    // 3. Verify the merge is for this task's branch
    let merge_branch = worktree::get_merge_branch(workspace_root)?;
    let expected = format!("bacchus/{}", task_id);

    if let Some(ref branch) = merge_branch {
        if branch != &expected {
            return Ok(ResolveOutput {
                success: false,
                task_id: task_id.to_string(),
                merged: false,
                message: format!(
                    "Current merge is for '{}', not '{}'. Resolve the correct task.",
                    branch, expected
                ),
            });
        }
    }

    // 4. Check for unresolved conflicts
    if worktree::has_unresolved_conflicts(workspace_root)? {
        return Ok(ResolveOutput {
            success: false,
            task_id: task_id.to_string(),
            merged: false,
            message: "Unresolved conflicts remain. Fix all conflicts and stage changes with 'git add'.".to_string(),
        });
    }

    // 5. Complete the merge
    worktree::complete_merge(workspace_root)?;

    // 6. Remove worktree (non-force since we merged)
    worktree::remove_worktree(workspace_root, task_id, false)?;

    // 7. Release SQLite task (marks as closed and clears claim)
    if !agent_id.is_empty() {
        tasks::release_sqlite_task(task_id, &agent_id)?;
    } else {
        tasks::update_sqlite_task_status(task_id, SqliteTaskStatus::Closed)?;
    }

    Ok(ResolveOutput {
        success: true,
        task_id: task_id.to_string(),
        merged: true,
        message: format!("Merge completed for {}. Worktree removed, task closed.", task_id),
    })
}
