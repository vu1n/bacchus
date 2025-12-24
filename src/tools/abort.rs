//! Abort tool - abort a failed merge for a bead
//!
//! Restores the repository to pre-merge state when a merge conflict occurs.

use crate::db::with_db;
use crate::worktree;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct AbortOutput {
    pub success: bool,
    pub bead_id: String,
    pub message: String,
}

pub fn abort_merge(
    bead_id: &str,
    workspace_root: &Path,
) -> Result<AbortOutput, Box<dyn std::error::Error>> {
    // 1. Check claim exists
    let claim_exists = with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM claims WHERE bead_id = ?1",
                [bead_id],
                |_| Ok(true),
            )
            .unwrap_or(false))
    })?;

    if !claim_exists {
        return Ok(AbortOutput {
            success: false,
            bead_id: bead_id.to_string(),
            message: format!("No claim found for {}", bead_id),
        });
    }

    // 2. Check we're in a merge conflict state
    if !worktree::is_in_merge_conflict(workspace_root)? {
        return Ok(AbortOutput {
            success: false,
            bead_id: bead_id.to_string(),
            message: "Not in a merge conflict state. Nothing to abort.".to_string(),
        });
    }

    // 3. Verify the merge is for this bead's branch
    let merge_branch = worktree::get_merge_branch(workspace_root)?;
    let expected = format!("bacchus/{}", bead_id);

    if let Some(ref branch) = merge_branch {
        if branch != &expected {
            return Ok(AbortOutput {
                success: false,
                bead_id: bead_id.to_string(),
                message: format!(
                    "Current merge conflict is for '{}', not '{}'. Abort the correct bead.",
                    branch, expected
                ),
            });
        }
    }

    // 4. Abort the merge
    worktree::abort_merge(workspace_root)?;

    Ok(AbortOutput {
        success: true,
        bead_id: bead_id.to_string(),
        message: format!(
            "Aborted merge for {}. Worktree preserved at .bacchus/worktrees/{}. Continue working or release with --status failed.",
            bead_id, bead_id
        ),
    })
}
