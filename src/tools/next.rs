//! Next task tool - gets ready task, creates worktree, claims it
//!
//! Combines task querying, worktree creation, and claiming in one operation.
//!
//! Prefers SQLite tasks (tasks_v2 table) over YAML tasks for atomic claiming.
//! Falls back to YAML with deprecation warning if no SQLite tasks available.

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
    /// Source of the task (sqlite or yaml)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Deprecation warning for YAML tasks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation_warning: Option<String>,
}

pub fn next_task(agent_id: &str, workspace_root: &Path) -> Result<NextOutput> {
    // Try SQLite tasks first (preferred)
    if let Some(result) = try_next_sqlite_task(agent_id, workspace_root)? {
        return Ok(result);
    }

    // Fall back to YAML tasks
    next_yaml_task(agent_id, workspace_root)
}

/// Try to claim the next ready SQLite task
fn try_next_sqlite_task(agent_id: &str, workspace_root: &Path) -> Result<Option<NextOutput>> {
    // Atomic claim: claim_next_sqlite_task handles readiness check and claim in one transaction
    match tasks::claim_next_sqlite_task(agent_id) {
        Ok(Some(task)) => {
            // Create worktree for the claimed task
            let wt = match worktree::create_worktree(workspace_root, &task.id) {
                Ok(wt) => wt,
                Err(e) => {
                    // Rollback: release the SQLite claim
                    let _ = tasks::release_sqlite_task(&task.id, agent_id);
                    return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(format!("Failed to create worktree: {}", e)),
                    ));
                }
            };

            Ok(Some(NextOutput {
                success: true,
                task_id: Some(task.id.clone()),
                title: Some(task.title),
                description: task.description,
                worktree_path: Some(wt.path.to_string_lossy().to_string()),
                branch: Some(wt.branch),
                message: format!("Claimed {} - work in {}", task.id, wt.path.display()),
                source: Some("sqlite".to_string()),
                deprecation_warning: None,
            }))
        }
        Ok(None) => {
            // No SQLite tasks ready, will fall back to YAML
            Ok(None)
        }
        Err(e) => {
            // Error querying SQLite, log and fall back to YAML
            eprintln!("Warning: SQLite task query failed: {}", e);
            Ok(None)
        }
    }
}

/// Claim the next ready YAML task (legacy path)
fn next_yaml_task(agent_id: &str, workspace_root: &Path) -> Result<NextOutput> {
    let deprecation_warning = Some(
        "YAML-based tasks are deprecated. Run 'bacchus task import' to migrate to SQLite.".to_string()
    );

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
            source: None,
            deprecation_warning: None,
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
            source: Some("yaml".to_string()),
            deprecation_warning,
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
        source: Some("yaml".to_string()),
        deprecation_warning,
    })
}
