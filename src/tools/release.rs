//! Release task tool - merges or discards worktree, updates task status
//!
//! Handles completing, blocking, or failing a claimed task.

use crate::db::with_db;
use crate::tasks;
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
    // 1. Check claim exists
    let claim_exists = with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM claims WHERE bead_id = ?1",
                [task_id],
                |_| Ok(true),
            )
            .unwrap_or(false))
    })?;

    if !claim_exists {
        return Ok(ReleaseOutput {
            success: false,
            task_id: task_id.to_string(),
            status: status.to_string(),
            merged: false,
            message: format!("No claim found for {}", task_id),
        });
    }

    let mut merged = false;

    match status {
        "done" => {
            // Merge worktree branch to main, then cleanup
            if let Err(e) = worktree::merge_worktree(workspace_root, task_id, "main") {
                // Check if this is a merge conflict
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

            // Remove worktree (non-force since we merged)
            worktree::remove_worktree(workspace_root, task_id, false)?;

            // Update task status
            tasks::update_task_status(workspace_root, task_id, "closed")?;
        }
        "blocked" => {
            // Keep worktree but release claim, mark task as blocked
            // Don't remove worktree - might want to resume later
            tasks::update_task_status(workspace_root, task_id, "blocked")?;
        }
        "failed" => {
            // Discard worktree (force), reset task to open for retry
            worktree::remove_worktree(workspace_root, task_id, true)?;
            tasks::update_task_status(workspace_root, task_id, "open")?;
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

    // Clear active footprints for this task
    if let Err(e) = tasks::clear_active_footprints(task_id) {
        eprintln!("Warning: Failed to clear footprints for {}: {}", task_id, e);
    }

    // Remove claim from DB
    with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [task_id]))?;

    Ok(ReleaseOutput {
        success: true,
        task_id: task_id.to_string(),
        status: status.to_string(),
        merged,
        message: format!("Released {} with status {}", task_id, status),
    })
}
