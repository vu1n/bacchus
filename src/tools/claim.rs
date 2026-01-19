//! Claim task tool - claims a specific task by ID, creates worktree
//!
//! Unlike `next`, this claims a specific task rather than the next ready one.
//! By default, only claims ready tasks (open, no blockers). Use --force to override.
//!
//! Supports both SQLite tasks (preferred) and YAML tasks (legacy, with deprecation warning).

use crate::db::with_db;
use crate::tasks::{self, TaskSource, SqliteTaskStatus};
use crate::worktree;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimOutput {
    pub success: bool,
    pub task_id: String,
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

pub fn claim_task(task_id: &str, agent_id: &str, force: bool, workspace_root: &Path) -> Result<ClaimOutput> {
    // Detect task source
    let source = tasks::detect_task_source(task_id, workspace_root);

    match source {
        TaskSource::Sqlite => claim_sqlite_task_internal(task_id, agent_id, force, workspace_root),
        TaskSource::Yaml => claim_yaml_task_internal(task_id, agent_id, force, workspace_root),
        TaskSource::NotFound => Ok(ClaimOutput {
            success: false,
            task_id: task_id.to_string(),
            title: None,
            description: None,
            worktree_path: None,
            branch: None,
            message: format!("Task {} not found in SQLite or YAML", task_id),
            source: None,
            deprecation_warning: None,
        }),
    }
}

/// Helper to convert TasksError to rusqlite::Error
fn tasks_error_to_rusqlite(e: tasks::TasksError) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(1),
        Some(e.to_string()),
    )
}

/// Claim a SQLite task (preferred path)
fn claim_sqlite_task_internal(task_id: &str, agent_id: &str, force: bool, workspace_root: &Path) -> Result<ClaimOutput> {
    // Use the atomic SQLite claim function
    let claim_result: std::result::Result<tasks::SqliteTask, tasks::TasksError> = if force {
        // Force claim bypasses readiness check - manually update status
        // First get the task to verify it exists
        let task = tasks::get_sqlite_task(task_id).map_err(tasks_error_to_rusqlite)?;

        if task.status == SqliteTaskStatus::Closed {
            return Ok(ClaimOutput {
                success: false,
                task_id: task_id.to_string(),
                title: Some(task.title),
                description: task.description,
                worktree_path: None,
                branch: None,
                message: format!("Task {} is already closed", task_id),
                source: Some("sqlite".to_string()),
                deprecation_warning: None,
            });
        }

        // For force claim, we need to directly claim without readiness check
        // Use the regular claim but catch the "not ready" error and force through
        match tasks::claim_sqlite_task(task_id, agent_id) {
            Ok(t) => Ok(t),
            Err(tasks::TasksError::NotReady(_)) => {
                // Force claim: manually update to in_progress
                tasks::update_sqlite_task_status(task_id, SqliteTaskStatus::InProgress)
                    .map_err(tasks_error_to_rusqlite)?;
                // Now get the updated task
                tasks::get_sqlite_task(task_id)
            }
            Err(e) => Err(e),
        }
    } else {
        tasks::claim_sqlite_task(task_id, agent_id)
    };

    match claim_result {
        Ok(task) => {
            // Create worktree for the task
            let wt = worktree::create_worktree(workspace_root, task_id).map_err(|e| {
                // Rollback: release the SQLite claim
                let _ = tasks::release_sqlite_task(task_id, agent_id);
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(format!("Failed to create worktree: {}", e)),
                )
            })?;

            Ok(ClaimOutput {
                success: true,
                task_id: task.id,
                title: Some(task.title),
                description: task.description,
                worktree_path: Some(wt.path.to_string_lossy().to_string()),
                branch: Some(wt.branch),
                message: format!("Claimed {} - work in {}", task_id, wt.path.display()),
                source: Some("sqlite".to_string()),
                deprecation_warning: None,
            })
        }
        Err(tasks::TasksError::NotReady(msg)) => {
            let task = tasks::get_sqlite_task(task_id).ok();
            Ok(ClaimOutput {
                success: false,
                task_id: task_id.to_string(),
                title: task.as_ref().map(|t| t.title.clone()),
                description: task.and_then(|t| t.description),
                worktree_path: None,
                branch: None,
                message: format!(
                    "Task {} is not ready: {}. Use --force to override.",
                    task_id, msg
                ),
                source: Some("sqlite".to_string()),
                deprecation_warning: None,
            })
        }
        Err(tasks::TasksError::TaskNotFound(_)) => Ok(ClaimOutput {
            success: false,
            task_id: task_id.to_string(),
            title: None,
            description: None,
            worktree_path: None,
            branch: None,
            message: format!("Task {} not found in SQLite", task_id),
            source: Some("sqlite".to_string()),
            deprecation_warning: None,
        }),
        Err(e) => Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to claim SQLite task: {}", e)),
        )),
    }
}

/// Claim a YAML task (legacy path with deprecation warning)
fn claim_yaml_task_internal(task_id: &str, agent_id: &str, force: bool, workspace_root: &Path) -> Result<ClaimOutput> {
    let deprecation_warning = Some(
        "YAML-based tasks are deprecated. Run 'bacchus task import' to migrate to SQLite.".to_string()
    );

    // 1. Get task details from tasks.yaml
    let task = tasks::get_task(workspace_root, task_id).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to get task: {}", e)),
        )
    })?;

    // 2. Check if task is closed (never claimable)
    if task.status == "closed" {
        return Ok(ClaimOutput {
            success: false,
            task_id: task_id.to_string(),
            title: Some(task.title),
            description: task.description,
            worktree_path: None,
            branch: None,
            message: format!("Task {} is already closed", task_id),
            source: Some("yaml".to_string()),
            deprecation_warning,
        });
    }

    // 3. Check if task is ready (unless --force)
    if !force {
        let is_ready = tasks::is_task_ready(workspace_root, task_id).map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Failed to check task readiness: {}", e)),
            )
        })?;

        if !is_ready {
            return Ok(ClaimOutput {
                success: false,
                task_id: task_id.to_string(),
                title: Some(task.title.clone()),
                description: task.description.clone(),
                worktree_path: None,
                branch: None,
                message: format!(
                    "Task {} is not ready (status: {}, may be blocked by dependencies or footprint collision). Use --force to override.",
                    task_id, task.status
                ),
                source: Some("yaml".to_string()),
                deprecation_warning,
            });
        }
    }

    // 4. Check if already claimed in bacchus DB
    let already_claimed = with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM claims WHERE bead_id = ?1",
                [task_id],
                |_| Ok(true),
            )
            .unwrap_or(false))
    })?;

    if already_claimed {
        return Ok(ClaimOutput {
            success: false,
            task_id: task_id.to_string(),
            title: Some(task.title),
            description: task.description,
            worktree_path: None,
            branch: None,
            message: format!("Task {} is already claimed", task_id),
            source: Some("yaml".to_string()),
            deprecation_warning,
        });
    }

    // 5. Create worktree
    let wt = worktree::create_worktree(workspace_root, task_id).map_err(|e| {
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
                task_id,
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
        let _ = worktree::remove_worktree(workspace_root, task_id, true);
        return Err(e);
    }

    // 7. Store active footprints for collision detection
    if let Err(e) = tasks::store_active_footprints(task_id, &task.footprint) {
        // Non-fatal: log warning but continue
        eprintln!("Warning: Failed to store footprints for {}: {}", task_id, e);
    }

    // 8. Update task status to in_progress (with rollback on failure)
    let status_result = tasks::update_task_status(workspace_root, task_id, "in_progress");

    if let Err(e) = status_result {
        // Rollback: remove worktree, claim, and footprints
        let _ = worktree::remove_worktree(workspace_root, task_id, true);
        let _ = with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [task_id]));
        let _ = tasks::clear_active_footprints(task_id);
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to update task status: {}", e)),
        ));
    }

    Ok(ClaimOutput {
        success: true,
        task_id: task_id.to_string(),
        title: Some(task.title),
        description: task.description,
        worktree_path: Some(wt.path.to_string_lossy().to_string()),
        branch: Some(wt.branch),
        message: format!("Claimed {} - work in {}", task_id, wt.path.display()),
        source: Some("yaml".to_string()),
        deprecation_warning,
    })
}
