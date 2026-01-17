//! Next task tool - gets ready task, creates worktree, claims it
//!
//! Combines task querying, worktree creation, and claiming in one operation.

use crate::db::with_db;
use crate::tasks;
use crate::worktree;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct NextOutput {
    pub success: bool,
    pub task_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub message: String,
}

pub fn next_task(agent_id: &str, workspace_root: &Path) -> Result<NextOutput> {
    // 1. Get ready tasks from tasks.yaml
    let ready = tasks::get_ready_tasks(workspace_root).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to get ready tasks: {}", e)),
        )
    })?;

    if ready.is_empty() {
        return Ok(NextOutput {
            success: false,
            task_id: None,
            title: None,
            description: None,
            worktree_path: None,
            branch: None,
            message: "No ready tasks available".to_string(),
        });
    }

    // 2. Pick first ready task (already sorted by priority)
    let task = &ready[0];

    // 3. Check if already claimed in bacchus DB
    let already_claimed = with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM claims WHERE bead_id = ?1",
                [&task.id],
                |_| Ok(true),
            )
            .unwrap_or(false))
    })?;

    if already_claimed {
        return Ok(NextOutput {
            success: false,
            task_id: Some(task.id.clone()),
            title: Some(task.title.clone()),
            description: task.description.clone(),
            worktree_path: None,
            branch: None,
            message: format!("Task {} is already claimed", task.id),
        });
    }

    // 4. Create worktree
    let wt = worktree::create_worktree(workspace_root, &task.id).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to create worktree: {}", e)),
        )
    })?;

    // 5. Record claim in bacchus DB (with rollback on failure)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let claim_result = with_db(|conn| {
        conn.execute(
            "INSERT INTO claims (bead_id, agent_id, worktree_path, branch_name, start_commit, claimed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                &task.id,
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
        let _ = worktree::remove_worktree(workspace_root, &task.id, true);
        return Err(e);
    }

    // 6. Store active footprints for collision detection
    if let Err(e) = tasks::store_active_footprints(&task.id, &task.footprint) {
        // Non-fatal: log warning but continue
        eprintln!("Warning: Failed to store footprints for {}: {}", task.id, e);
    }

    // 7. Update task status to in_progress (with rollback on failure)
    let status_result = tasks::update_task_status(workspace_root, &task.id, "in_progress");

    if let Err(e) = status_result {
        // Rollback: remove worktree, claim, and footprints
        let _ = worktree::remove_worktree(workspace_root, &task.id, true);
        let _ = with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [&task.id]));
        let _ = tasks::clear_active_footprints(&task.id);
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to update task status: {}", e)),
        ));
    }

    Ok(NextOutput {
        success: true,
        task_id: Some(task.id.clone()),
        title: Some(task.title.clone()),
        description: task.description.clone(),
        worktree_path: Some(wt.path.to_string_lossy().to_string()),
        branch: Some(wt.branch),
        message: format!("Claimed {} - work in {}", task.id, wt.path.display()),
    })
}
