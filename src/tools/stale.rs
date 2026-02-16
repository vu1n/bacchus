//! Stale claims tool - finds and optionally cleans up abandoned claims
//!
//! Detects claims older than a threshold and can clean them up.
//! Uses SQLite-based task management (tasks table).

use crate::db::with_db;
use crate::events;
use crate::tasks::{self, SqliteTaskStatus};
use crate::workspace;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct StaleClaim {
    pub task_id: String,
    pub agent_id: String,
    pub workspace_path: String,
    pub claimed_at: i64,
    pub claimed_heartbeat_at: Option<i64>,
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

    // Query SQLite tasks for stale claims, using heartbeat as the source of truth.
    let stale_claims: Vec<StaleClaim> = with_db(|conn| {
        // Find tasks that are in_progress with old claims
        let mut stmt = conn.prepare(
            "SELECT id, claimed_by, claimed_at, claimed_heartbeat_at FROM tasks
             WHERE status = 'in_progress'
             AND deleted_at IS NULL
             AND (COALESCE(claimed_heartbeat_at, claimed_at) IS NULL OR COALESCE(claimed_heartbeat_at, claimed_at) < ?1)",
        )?;

        let claims = stmt
            .query_map([cutoff], |row| {
                let claimed_at: Option<i64> = row.get(2)?;
                let claimed_at = claimed_at.unwrap_or(0);
                let claimed_heartbeat_at: Option<i64> = row.get(3)?;
                let last_seen = claimed_heartbeat_at.unwrap_or(claimed_at);
                let task_id: String = row.get(0)?;

                let workspace_path = format!(".bacchus/workspaces/{}", task_id);

                Ok(StaleClaim {
                    task_id,
                    agent_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    workspace_path,
                    claimed_at,
                    claimed_heartbeat_at,
                    age_minutes: if last_seen > 0 {
                        (now - last_seen) / 60000
                    } else {
                        0
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(claims)
    })?;

    let mut cleaned_up = Vec::new();

    if cleanup {
        for claim in &stale_claims {
            // Remove jj workspace (force to discard any changes)
            if let Err(e) = workspace::remove_workspace(workspace_root, &claim.task_id, true) {
                eprintln!(
                    "Warning: Failed to remove workspace for {}: {}",
                    claim.task_id, e
                );
                // Continue anyway - workspace might not exist
            }

            // Reset SQLite task to open
            if let Err(e) = tasks::reset_sqlite_task(&claim.task_id, SqliteTaskStatus::Open) {
                eprintln!(
                    "Warning: Failed to reset SQLite task status for {}: {}",
                    claim.task_id, e
                );
                continue;
            }
            let _ = events::record_event(
                None,
                "orchestrator",
                "stale_claim_cleaned",
                "task",
                &claim.task_id,
                &serde_json::json!({
                    "agent_id": claim.agent_id,
                    "age_minutes": claim.age_minutes
                }),
                Some(&format!("stale-cleanup:{}", claim.task_id)),
            );

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
