//! Claim task tool - claims a specific bead by ID, creates worktree
//!
//! Unlike `next`, this claims a specific bead rather than the next ready one.
//! By default, only claims ready beads (open, no blockers). Use --force to override.

use crate::beads;
use crate::db::with_db;
use crate::worktree;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimOutput {
    pub success: bool,
    pub bead_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub message: String,
}

pub fn claim_task(bead_id: &str, agent_id: &str, force: bool, workspace_root: &Path) -> Result<ClaimOutput> {
    // 1. Get bead details from beads DB
    let bead = beads::get_bead(bead_id).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to get bead: {}", e)),
        )
    })?;

    // 2. Check if bead is closed (never claimable)
    if bead.status == "closed" {
        return Ok(ClaimOutput {
            success: false,
            bead_id: bead_id.to_string(),
            title: Some(bead.title),
            description: bead.description,
            worktree_path: None,
            branch: None,
            message: format!("Bead {} is already closed", bead_id),
        });
    }

    // 3. Check if bead is ready (unless --force)
    if !force {
        let is_ready = beads::is_bead_ready(bead_id).map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Failed to check bead readiness: {}", e)),
            )
        })?;

        if !is_ready {
            return Ok(ClaimOutput {
                success: false,
                bead_id: bead_id.to_string(),
                title: Some(bead.title.clone()),
                description: bead.description.clone(),
                worktree_path: None,
                branch: None,
                message: format!(
                    "Bead {} is not ready (status: {}, may be blocked by dependencies). Use --force to override.",
                    bead_id, bead.status
                ),
            });
        }
    }

    // 4. Check if already claimed in bacchus DB
    let already_claimed = with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM claims WHERE bead_id = ?1",
                [bead_id],
                |_| Ok(true),
            )
            .unwrap_or(false))
    })?;

    if already_claimed {
        return Ok(ClaimOutput {
            success: false,
            bead_id: bead_id.to_string(),
            title: Some(bead.title),
            description: bead.description,
            worktree_path: None,
            branch: None,
            message: format!("Bead {} is already claimed", bead_id),
        });
    }

    // 5. Create worktree
    let wt = worktree::create_worktree(workspace_root, bead_id).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to create worktree: {}", e)),
        )
    })?;

    // 6. Record claim in bacchus DB (with rollback on failure)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let claim_result = with_db(|conn| {
        conn.execute(
            "INSERT INTO claims (bead_id, agent_id, worktree_path, branch_name, start_commit, claimed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                bead_id,
                agent_id,
                wt.path.to_string_lossy().to_string(),
                &wt.branch,
                &wt.head_commit,
                now
            ],
        )
    });

    if let Err(e) = claim_result {
        // Rollback: remove orphaned worktree
        let _ = worktree::remove_worktree(workspace_root, bead_id, true);
        return Err(e);
    }

    // 7. Update bead status to in_progress (with rollback on failure)
    let status_result = beads::update_bead_status(bead_id, "in_progress");

    if let Err(e) = status_result {
        // Rollback: remove worktree and claim
        let _ = worktree::remove_worktree(workspace_root, bead_id, true);
        let _ = with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [bead_id]));
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to update bead status: {}", e)),
        ));
    }

    Ok(ClaimOutput {
        success: true,
        bead_id: bead_id.to_string(),
        title: Some(bead.title),
        description: bead.description,
        worktree_path: Some(wt.path.to_string_lossy().to_string()),
        branch: Some(wt.branch),
        message: format!("Claimed {} - work in {}", bead_id, wt.path.display()),
    })
}
