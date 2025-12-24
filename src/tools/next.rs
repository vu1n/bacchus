//! Next task tool - gets ready bead, creates worktree, claims it
//!
//! Combines beads querying, worktree creation, and claiming in one operation.

use crate::beads;
use crate::db::with_db;
use crate::worktree;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct NextOutput {
    pub success: bool,
    pub bead_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub message: String,
}

pub fn next_task(agent_id: &str, workspace_root: &Path) -> Result<NextOutput> {
    // 1. Get ready beads from beads DB
    let ready = beads::get_ready_beads(workspace_root).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to get ready beads: {}", e)),
        )
    })?;

    if ready.is_empty() {
        return Ok(NextOutput {
            success: false,
            bead_id: None,
            title: None,
            description: None,
            worktree_path: None,
            branch: None,
            message: "No ready beads available".to_string(),
        });
    }

    // 2. Pick first ready bead (already sorted by priority)
    let bead = &ready[0];

    // 3. Check if already claimed in bacchus DB
    let already_claimed = with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM claims WHERE bead_id = ?1",
                [&bead.id],
                |_| Ok(true),
            )
            .unwrap_or(false))
    })?;

    if already_claimed {
        return Ok(NextOutput {
            success: false,
            bead_id: Some(bead.id.clone()),
            title: Some(bead.title.clone()),
            description: bead.description.clone(),
            worktree_path: None,
            branch: None,
            message: format!("Bead {} is already claimed", bead.id),
        });
    }

    // 4. Create worktree
    let wt = worktree::create_worktree(workspace_root, &bead.id).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to create worktree: {}", e)),
        )
    })?;

    // 5. Record claim in bacchus DB
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    with_db(|conn| {
        conn.execute(
            "INSERT INTO claims (bead_id, agent_id, worktree_path, branch_name, start_commit, claimed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                &bead.id,
                agent_id,
                wt.path.to_string_lossy().to_string(),
                &wt.branch,
                &wt.head_commit,
                now
            ],
        )
    })?;

    // 6. Update bead status to in_progress
    beads::update_bead_status(workspace_root, &bead.id, "in_progress").map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to update bead status: {}", e)),
        )
    })?;

    Ok(NextOutput {
        success: true,
        bead_id: Some(bead.id.clone()),
        title: Some(bead.title.clone()),
        description: bead.description.clone(),
        worktree_path: Some(wt.path.to_string_lossy().to_string()),
        branch: Some(wt.branch),
        message: format!("Claimed {} - work in {}", bead.id, wt.path.display()),
    })
}
