//! Stale claims tool - finds and optionally cleans up abandoned claims
//!
//! Detects claims older than a threshold and can clean them up.
//!
//! Queries both:
//! - Legacy claims table (for YAML tasks)
//! - SQLite tasks_v2.claimed_by with lease_expires_at (for SQLite tasks)

use crate::db::with_db;
use crate::tasks::{self, TaskSource, SqliteTaskStatus};
use crate::worktree;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct StaleClaim {
    pub task_id: String,
    pub agent_id: String,
    pub worktree_path: String,
    pub claimed_at: i64,
    pub age_minutes: i64,
    /// Source of the claim (sqlite or yaml/legacy)
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StaleOutput {
    pub stale_claims: Vec<StaleClaim>,
    pub cleaned_up: Vec<String>,
    pub message: String,
}

pub fn find_stale(
    minutes: i64,
    cleanup: bool,
    workspace_root: &Path,
) -> Result<StaleOutput, Box<dyn std::error::Error>> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let threshold_ms = minutes * 60 * 1000;
    let cutoff = now - threshold_ms;

    // Find stale claims from BOTH sources
    let mut stale_claims: Vec<StaleClaim> = Vec::new();

    // 1. Query legacy claims table (for YAML tasks and old SQLite claims)
    let legacy_claims: Vec<StaleClaim> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT bead_id, agent_id, worktree_path, claimed_at FROM claims WHERE claimed_at < ?1",
        )?;

        let claims = stmt
            .query_map([cutoff], |row| {
                let claimed_at: i64 = row.get(3)?;
                Ok(StaleClaim {
                    task_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    worktree_path: row.get(2)?,
                    claimed_at,
                    age_minutes: (now - claimed_at) / 60000,
                    source: "legacy".to_string(),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(claims)
    })?;

    stale_claims.extend(legacy_claims);

    // 2. Query SQLite tasks_v2 for stale claims (expired lease or old heartbeat)
    let sqlite_claims: Vec<StaleClaim> = with_db(|conn| {
        // Find tasks that are in_progress with expired leases
        let mut stmt = conn.prepare(
            "SELECT id, claimed_by, claimed_at, lease_expires_at FROM tasks_v2
             WHERE status = 'in_progress'
             AND deleted_at IS NULL
             AND (lease_expires_at < ?1 OR (lease_expires_at IS NULL AND claimed_at < ?2))",
        )?;

        let claims = stmt
            .query_map([now, cutoff], |row| {
                let claimed_at: Option<i64> = row.get(2)?;
                let claimed_at = claimed_at.unwrap_or(0);
                let task_id: String = row.get(0)?;

                // Try to get worktree path from legacy claims table, or construct it
                let worktree_path = format!(".bacchus/worktrees/{}", task_id);

                Ok(StaleClaim {
                    task_id,
                    agent_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    worktree_path,
                    claimed_at,
                    age_minutes: if claimed_at > 0 { (now - claimed_at) / 60000 } else { 0 },
                    source: "sqlite".to_string(),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(claims)
    })?;

    // Add SQLite claims, avoiding duplicates (same task_id already in legacy)
    let legacy_task_ids: std::collections::HashSet<_> =
        stale_claims.iter().map(|c| c.task_id.clone()).collect();

    for claim in sqlite_claims {
        if !legacy_task_ids.contains(&claim.task_id) {
            stale_claims.push(claim);
        }
    }

    let mut cleaned_up = Vec::new();

    if cleanup {
        for claim in &stale_claims {
            // Remove worktree (force to discard any changes)
            if let Err(e) = worktree::remove_worktree(workspace_root, &claim.task_id, true) {
                eprintln!(
                    "Warning: Failed to remove worktree for {}: {}",
                    claim.task_id, e
                );
                // Continue anyway - worktree might not exist
            }

            // Clean up based on source
            match tasks::detect_task_source(&claim.task_id, workspace_root) {
                TaskSource::Sqlite => {
                    // Reset SQLite task to open
                    if let Err(e) = tasks::update_sqlite_task_status(&claim.task_id, SqliteTaskStatus::Open) {
                        eprintln!(
                            "Warning: Failed to reset SQLite task status for {}: {}",
                            claim.task_id, e
                        );
                    }
                    // Also reclaim stale SQLite tasks
                    let _ = tasks::reclaim_stale_sqlite_tasks();
                }
                TaskSource::Yaml => {
                    // Clear active footprints for this task
                    if let Err(e) = tasks::clear_active_footprints(&claim.task_id) {
                        eprintln!(
                            "Warning: Failed to clear footprints for {}: {}",
                            claim.task_id, e
                        );
                    }

                    // Reset YAML task status to open for retry
                    if let Err(e) = tasks::update_task_status(workspace_root, &claim.task_id, "open") {
                        eprintln!(
                            "Warning: Failed to reset task status for {}: {}",
                            claim.task_id, e
                        );
                    }
                }
                TaskSource::NotFound => {
                    // Task was deleted, just clean up the claim
                    eprintln!(
                        "Warning: Task {} not found (may have been deleted)",
                        claim.task_id
                    );
                }
            }

            // Remove from legacy claims table if present
            let _ = with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [&claim.task_id]));
            let _ = tasks::clear_active_footprints(&claim.task_id);

            cleaned_up.push(claim.task_id.clone());
        }
    }

    let message = if cleanup {
        format!(
            "Found {} stale claims, cleaned up {}",
            stale_claims.len(),
            cleaned_up.len()
        )
    } else {
        format!(
            "Found {} stale claims (use --cleanup to remove)",
            stale_claims.len()
        )
    };

    Ok(StaleOutput {
        stale_claims,
        cleaned_up,
        message,
    })
}
