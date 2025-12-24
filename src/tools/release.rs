//! Release task tool - merges or discards worktree, updates bead status
//!
//! Handles completing, blocking, or failing a claimed bead.

use crate::beads;
use crate::db::with_db;
use crate::worktree;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseOutput {
    pub success: bool,
    pub bead_id: String,
    pub status: String,
    pub merged: bool,
    pub message: String,
}

pub fn release_bead(
    bead_id: &str,
    status: &str,
    workspace_root: &Path,
) -> Result<ReleaseOutput, Box<dyn std::error::Error>> {
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
        return Ok(ReleaseOutput {
            success: false,
            bead_id: bead_id.to_string(),
            status: status.to_string(),
            merged: false,
            message: format!("No claim found for {}", bead_id),
        });
    }

    let mut merged = false;

    match status {
        "done" => {
            // Merge worktree branch to main, then cleanup
            if let Err(e) = worktree::merge_worktree(workspace_root, bead_id, "main") {
                return Ok(ReleaseOutput {
                    success: false,
                    bead_id: bead_id.to_string(),
                    status: status.to_string(),
                    merged: false,
                    message: format!("Failed to merge: {}", e),
                });
            }
            merged = true;

            // Remove worktree (non-force since we merged)
            worktree::remove_worktree(workspace_root, bead_id, false)?;

            // Update bead status
            beads::update_bead_status(workspace_root, bead_id, "closed")?;
        }
        "blocked" => {
            // Keep worktree but release claim, mark bead as blocked
            // Don't remove worktree - might want to resume later
            beads::update_bead_status(workspace_root, bead_id, "blocked")?;
        }
        "failed" => {
            // Discard worktree (force), reset bead to open for retry
            worktree::remove_worktree(workspace_root, bead_id, true)?;
            beads::update_bead_status(workspace_root, bead_id, "open")?;
        }
        _ => {
            return Ok(ReleaseOutput {
                success: false,
                bead_id: bead_id.to_string(),
                status: status.to_string(),
                merged: false,
                message: format!("Invalid status: {}. Use done, blocked, or failed", status),
            });
        }
    }

    // Remove claim from DB
    with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [bead_id]))?;

    Ok(ReleaseOutput {
        success: true,
        bead_id: bead_id.to_string(),
        status: status.to_string(),
        merged,
        message: format!("Released {} with status {}", bead_id, status),
    })
}
