//! Stale claims tool - finds and optionally cleans up abandoned claims
//!
//! Detects claims older than a threshold and can clean them up.

use crate::beads;
use crate::db::with_db;
use crate::worktree;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize)]
pub struct StaleClaim {
    pub bead_id: String,
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

    // Find stale claims
    let stale_claims: Vec<StaleClaim> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT bead_id, agent_id, worktree_path, claimed_at FROM claims WHERE claimed_at < ?1",
        )?;

        let claims = stmt
            .query_map([cutoff], |row| {
                let claimed_at: i64 = row.get(3)?;
                Ok(StaleClaim {
                    bead_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    worktree_path: row.get(2)?,
                    claimed_at,
                    age_minutes: (now - claimed_at) / 60000,
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
            if let Err(e) = worktree::remove_worktree(workspace_root, &claim.bead_id, true) {
                eprintln!(
                    "Warning: Failed to remove worktree for {}: {}",
                    claim.bead_id, e
                );
                // Continue anyway - worktree might not exist
            }

            // Reset bead status to open for retry
            if let Err(e) = beads::update_bead_status(&claim.bead_id, "open") {
                eprintln!(
                    "Warning: Failed to reset bead status for {}: {}",
                    claim.bead_id, e
                );
            }

            // Remove claim from DB
            let _ = with_db(|conn| conn.execute("DELETE FROM claims WHERE bead_id = ?1", [&claim.bead_id]));

            cleaned_up.push(claim.bead_id.clone());
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
