//! Release task tool - merges or discards worktree, updates task status
//!
//! Handles completing, blocking, or failing a claimed task.
//!
//! Supports both SQLite tasks (tasks_v2) and YAML tasks (legacy).
//! Detects the task source and routes to the appropriate release path.

use crate::db::with_db;
use crate::tasks::{self, TaskSource, SqliteTaskStatus};
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
    /// Source of the task (sqlite or yaml)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

pub fn release_bead(
    task_id: &str,
    status: &str,
    workspace_root: &Path,
) -> Result<ReleaseOutput, Box<dyn std::error::Error>> {
    // Detect task source
    let source = tasks::detect_task_source(task_id, workspace_root);

    // Also check if there's a claim in the legacy claims table
    let has_legacy_claim = with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM claims WHERE bead_id = ?1",
                [task_id],
                |_| Ok(true),
            )
            .unwrap_or(false))
    })?;

    // Check if SQLite task has a claimed_by
    let has_sqlite_claim = if source == TaskSource::Sqlite {
        tasks::get_sqlite_task(task_id)
            .map(|t| t.claimed_by.is_some())
            .unwrap_or(false)
    } else {
        false
    };

    if !has_legacy_claim && !has_sqlite_claim {
        return Ok(ReleaseOutput {
            success: false,
            task_id: task_id.to_string(),
            status: status.to_string(),
            merged: false,
            message: format!("No claim found for {}", task_id),
            source: None,
        });
    }

    match source {
        TaskSource::Sqlite => release_sqlite_task(task_id, status, workspace_root, has_legacy_claim),
        TaskSource::Yaml => release_yaml_task(task_id, status, workspace_root),
        TaskSource::NotFound => {
            // Task was deleted but claim exists - clean up
            if has_legacy_claim {
                release_orphaned_claim(task_id, workspace_root)
            } else {
                Ok(ReleaseOutput {
                    success: false,
                    task_id: task_id.to_string(),
                    status: status.to_string(),
                    merged: false,
                    message: format!("Task {} not found", task_id),
                    source: None,
                })
            }
        }
    }
}

/// Release a SQLite task
fn release_sqlite_task(
    task_id: &str,
    status: &str,
    workspace_root: &Path,
    has_legacy_claim: bool,
) -> Result<ReleaseOutput, Box<dyn std::error::Error>> {
    let mut merged = false;

    // Get agent_id from SQLite task claim
    let task = tasks::get_sqlite_task(task_id)?;
    let agent_id = task.claimed_by.clone().unwrap_or_default();

    match status {
        "done" => {
            // Merge worktree branch to main, then cleanup
            if let Err(e) = worktree::merge_worktree(workspace_root, task_id, "main") {
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
                    source: Some("sqlite".to_string()),
                });
            }
            merged = true;

            // Remove worktree
            worktree::remove_worktree(workspace_root, task_id, false)?;

            // Release SQLite task (marks as closed)
            if !agent_id.is_empty() {
                tasks::release_sqlite_task(task_id, &agent_id)?;
            } else {
                // No agent_id, manually close
                tasks::update_sqlite_task_status(task_id, SqliteTaskStatus::Closed)?;
            }
        }
        "blocked" => {
            // Keep worktree, mark as blocked
            tasks::update_sqlite_task_status(task_id, SqliteTaskStatus::Blocked)?;
        }
        "failed" => {
            // Discard worktree, reset to open
            worktree::remove_worktree(workspace_root, task_id, true)?;
            tasks::update_sqlite_task_status(task_id, SqliteTaskStatus::Open)?;
        }
        _ => {
            return Ok(ReleaseOutput {
                success: false,
                task_id: task_id.to_string(),
                status: status.to_string(),
                merged: false,
                message: format!("Invalid status: {}. Use done, blocked, or failed", status),
                source: Some("sqlite".to_string()),
            });
        }
    }

    // Clean up legacy claim table if present
    if has_legacy_claim {
        let _ = with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [task_id]));
        let _ = tasks::clear_active_footprints(task_id);
    }

    Ok(ReleaseOutput {
        success: true,
        task_id: task_id.to_string(),
        status: status.to_string(),
        merged,
        message: format!("Released {} with status {}", task_id, status),
        source: Some("sqlite".to_string()),
    })
}

/// Release a YAML task (legacy path)
fn release_yaml_task(
    task_id: &str,
    status: &str,
    workspace_root: &Path,
) -> Result<ReleaseOutput, Box<dyn std::error::Error>> {
    let mut merged = false;

    match status {
        "done" => {
            // Merge worktree branch to main, then cleanup
            if let Err(e) = worktree::merge_worktree(workspace_root, task_id, "main") {
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
                    source: Some("yaml".to_string()),
                });
            }
            merged = true;

            // Remove worktree
            worktree::remove_worktree(workspace_root, task_id, false)?;

            // Update YAML task status
            tasks::update_task_status(workspace_root, task_id, "closed")?;
        }
        "blocked" => {
            tasks::update_task_status(workspace_root, task_id, "blocked")?;
        }
        "failed" => {
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
                source: Some("yaml".to_string()),
            });
        }
    }

    // Clear legacy claim data
    if let Err(e) = tasks::clear_active_footprints(task_id) {
        eprintln!("Warning: Failed to clear footprints for {}: {}", task_id, e);
    }
    with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [task_id]))?;

    Ok(ReleaseOutput {
        success: true,
        task_id: task_id.to_string(),
        status: status.to_string(),
        merged,
        message: format!("Released {} with status {}", task_id, status),
        source: Some("yaml".to_string()),
    })
}

/// Release an orphaned claim (task was deleted but claim exists)
fn release_orphaned_claim(
    task_id: &str,
    workspace_root: &Path,
) -> Result<ReleaseOutput, Box<dyn std::error::Error>> {
    // Try to remove worktree if it exists
    let _ = worktree::remove_worktree(workspace_root, task_id, true);

    // Clear legacy claim data
    let _ = tasks::clear_active_footprints(task_id);
    with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [task_id]))?;

    Ok(ReleaseOutput {
        success: true,
        task_id: task_id.to_string(),
        status: "orphaned".to_string(),
        merged: false,
        message: format!("Released orphaned claim for {} (task was deleted)", task_id),
        source: None,
    })
}
