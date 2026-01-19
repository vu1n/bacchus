//! List active claims and worktrees
//!
//! Merges claims from both:
//! - Legacy claims table (for YAML tasks)
//! - SQLite tasks_v2.claimed_by (for SQLite tasks)

use crate::db::with_db;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize)]
pub struct ListOutput {
    pub claims: Vec<ClaimInfo>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimInfo {
    pub task_id: String,
    pub agent_id: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub age_minutes: i64,
    /// Source of the claim (sqlite or legacy)
    pub source: String,
}

/// List all active claims from both sources
pub fn list_claims() -> Result<ListOutput> {
    with_db(|conn| {
        let mut claims: Vec<ClaimInfo> = Vec::new();
        let mut seen_task_ids: HashSet<String> = HashSet::new();

        // 1. Query legacy claims table
        {
            let mut stmt = conn.prepare(
                "SELECT bead_id, agent_id, worktree_path, branch_name,
                        (strftime('%s', 'now') * 1000 - claimed_at) / 60000 as age_minutes
                 FROM claims
                 ORDER BY claimed_at DESC",
            )?;

            let legacy_claims: Vec<ClaimInfo> = stmt
                .query_map([], |row| {
                    Ok(ClaimInfo {
                        task_id: row.get(0)?,
                        agent_id: row.get(1)?,
                        worktree_path: row.get(2)?,
                        branch_name: row.get(3)?,
                        age_minutes: row.get(4)?,
                        source: "legacy".to_string(),
                    })
                })?
                .filter_map(|r| r.ok())
                .collect();

            for claim in legacy_claims {
                seen_task_ids.insert(claim.task_id.clone());
                claims.push(claim);
            }
        }

        // 2. Query SQLite tasks_v2 for in_progress tasks with claimed_by
        {
            let mut stmt = conn.prepare(
                "SELECT id, claimed_by, claimed_at
                 FROM tasks_v2
                 WHERE status = 'in_progress'
                   AND claimed_by IS NOT NULL
                   AND deleted_at IS NULL
                 ORDER BY claimed_at DESC",
            )?;

            let sqlite_claims: Vec<ClaimInfo> = stmt
                .query_map([], |row| {
                    let task_id: String = row.get(0)?;
                    let claimed_at: Option<i64> = row.get(2)?;
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let age_minutes = claimed_at.map(|ca| (now_ms - ca) / 60000).unwrap_or(0);

                    Ok(ClaimInfo {
                        task_id: task_id.clone(),
                        agent_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        worktree_path: format!(".bacchus/worktrees/{}", task_id),
                        branch_name: format!("bacchus/{}", task_id),
                        age_minutes,
                        source: "sqlite".to_string(),
                    })
                })?
                .filter_map(|r| r.ok())
                .collect();

            // Add SQLite claims, avoiding duplicates
            for claim in sqlite_claims {
                if !seen_task_ids.contains(&claim.task_id) {
                    seen_task_ids.insert(claim.task_id.clone());
                    claims.push(claim);
                }
            }
        }

        // Sort by age (most recent first)
        claims.sort_by(|a, b| a.age_minutes.cmp(&b.age_minutes));

        Ok(ListOutput {
            total: claims.len(),
            claims,
        })
    })
}
