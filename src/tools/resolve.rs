//! Resolve tool - complete a merge after manual conflict resolution
//!
//! Finishes the merge, removes worktree, and updates bead status.

use crate::beads;
use crate::db::with_db;
use crate::worktree;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveOutput {
    pub success: bool,
    pub bead_id: String,
    pub merged: bool,
    pub message: String,
}

pub fn resolve_merge(
    bead_id: &str,
    workspace_root: &Path,
) -> Result<ResolveOutput, Box<dyn std::error::Error>> {
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
        return Ok(ResolveOutput {
            success: false,
            bead_id: bead_id.to_string(),
            merged: false,
            message: format!("No claim found for {}", bead_id),
        });
    }

    // 2. Check we're in a merge state
    if !worktree::is_in_merge_conflict(workspace_root)? {
        return Ok(ResolveOutput {
            success: false,
            bead_id: bead_id.to_string(),
            merged: false,
            message: "Not in a merge state. Use 'bacchus release --status done' instead.".to_string(),
        });
    }

    // 3. Verify the merge is for this bead's branch
    let merge_branch = worktree::get_merge_branch(workspace_root)?;
    let expected = format!("bacchus/{}", bead_id);

    if let Some(ref branch) = merge_branch {
        if branch != &expected {
            return Ok(ResolveOutput {
                success: false,
                bead_id: bead_id.to_string(),
                merged: false,
                message: format!(
                    "Current merge is for '{}', not '{}'. Resolve the correct bead.",
                    branch, expected
                ),
            });
        }
    }

    // 4. Check for unresolved conflicts
    if worktree::has_unresolved_conflicts(workspace_root)? {
        return Ok(ResolveOutput {
            success: false,
            bead_id: bead_id.to_string(),
            merged: false,
            message: "Unresolved conflicts remain. Fix all conflicts and stage changes with 'git add'.".to_string(),
        });
    }

    // 5. Complete the merge
    worktree::complete_merge(workspace_root)?;

    // 6. Remove worktree (non-force since we merged)
    worktree::remove_worktree(workspace_root, bead_id, false)?;

    // 7. Update bead status
    beads::update_bead_status(bead_id, "closed")?;

    // 8. Remove claim
    with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [bead_id]))?;

    Ok(ResolveOutput {
        success: true,
        bead_id: bead_id.to_string(),
        merged: true,
        message: format!("Merge completed for {}. Worktree removed, bead closed.", bead_id),
    })
}
