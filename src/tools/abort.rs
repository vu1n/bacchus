//! Abort tool - abort a failed release for a task
//!
//! In jj workflow, this resets a task from needs_resolution back to in_progress
//! so the agent can continue working on it.

use crate::tasks::{self, SqliteTaskStatus, TasksError};
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::ToolError;

#[derive(Debug, Serialize, Deserialize)]
pub struct AbortOutput {
    pub success: bool,
    pub task_id: String,
    pub message: String,
}

pub fn abort_merge(task_id: &str, _workspace_root: &Path) -> Result<AbortOutput, ToolError> {
    // 1. Check task exists
    let task = match tasks::get_sqlite_task(task_id) {
        Ok(t) => t,
        Err(TasksError::TaskNotFound(_)) => {
            return Ok(AbortOutput {
                success: false,
                task_id: task_id.to_string(),
                message: format!("Task {} not found", task_id),
            });
        }
        Err(e) => return Err(e.into()),
    };

    // 2. Check task is in needs_resolution status
    if task.status != SqliteTaskStatus::NeedsResolution {
        return Ok(AbortOutput {
            success: false,
            task_id: task_id.to_string(),
            message: format!(
                "Task {} is not in needs_resolution status (current: {}). Nothing to abort.",
                task_id,
                task.status.as_str()
            ),
        });
    }

    // 3. Reset task from needs_resolution back to in_progress
    tasks::reset_task_from_resolution(task_id)?;

    Ok(AbortOutput {
        success: true,
        task_id: task_id.to_string(),
        message: format!(
            "Reset {} from needs_resolution to in_progress. Workspace preserved at .bacchus/workspaces/{}. Continue working.",
            task_id, task_id
        ),
    })
}
