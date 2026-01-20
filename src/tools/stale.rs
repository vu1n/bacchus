//! Stale claims tool - finds and optionally cleans up abandoned claims
//!
//! Detects claims older than a threshold and can clean them up.
//! Uses SQLite-based task management (tasks table).

use crate::db::with_db;
use crate::tasks::{self, SqliteTaskStatus};
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

    // Query SQLite tasks for stale claims (claimed_at threshold or legacy NULL claims)
    let stale_claims: Vec<StaleClaim> = with_db(|conn| {
        // Find tasks that are in_progress with old claims
        let mut stmt = conn.prepare(
            "SELECT id, claimed_by, claimed_at FROM tasks
             WHERE status = 'in_progress'
             AND deleted_at IS NULL
             AND (claimed_at IS NULL OR claimed_at < ?1)",
        )?;

        let claims = stmt
            .query_map([cutoff], |row| {
                let claimed_at: Option<i64> = row.get(2)?;
                let claimed_at = claimed_at.unwrap_or(0);
                let task_id: String = row.get(0)?;

                let worktree_path = format!(".bacchus/worktrees/{}", task_id);

                Ok(StaleClaim {
                    task_id,
                    agent_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    worktree_path,
                    claimed_at,
                    age_minutes: if claimed_at > 0 { (now - claimed_at) / 60000 } else { 0 },
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(claims)
    })?;

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

            // Reset SQLite task to open
            if let Err(e) = tasks::reset_sqlite_task(&claim.task_id, SqliteTaskStatus::Open) {
                eprintln!(
                    "Warning: Failed to reset SQLite task status for {}: {}",
                    claim.task_id, e
                );
            }

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
