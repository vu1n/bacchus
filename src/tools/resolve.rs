//! Resolve tool - complete a merge after manual conflict resolution
//!
//! Finishes the merge, removes worktree, and updates task status.

use crate::db::with_db;
use crate::tasks;
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
        return Ok(ResolveOutput {
            success: false,
            task_id: task_id.to_string(),
            merged: false,
            message: format!("No claim found for {}", task_id),
        });
    }

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

    // 7. Clear active footprints
    if let Err(e) = tasks::clear_active_footprints(task_id) {
        eprintln!("Warning: Failed to clear footprints for {}: {}", task_id, e);
    }

    // 8. Update task status
    tasks::update_task_status(workspace_root, task_id, "closed")?;

    // 9. Remove claim
    with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [task_id]))?;

    Ok(ResolveOutput {
        success: true,
        task_id: task_id.to_string(),
        merged: true,
        message: format!("Merge completed for {}. Worktree removed, task closed.", task_id),
    })
}
