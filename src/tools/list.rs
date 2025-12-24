//! List active claims and worktrees

use crate::db::with_db;
use rusqlite::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ListOutput {
    pub claims: Vec<ClaimInfo>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimInfo {
    pub bead_id: String,
    pub agent_id: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub age_minutes: i64,
}

/// List all active claims
pub fn list_claims() -> Result<ListOutput> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT bead_id, agent_id, worktree_path, branch_name,
                    (strftime('%s', 'now') * 1000 - claimed_at) / 60000 as age_minutes
             FROM claims
             ORDER BY claimed_at DESC",
        )?;

        let claims: Vec<ClaimInfo> = stmt
            .query_map([], |row| {
                Ok(ClaimInfo {
                    bead_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    worktree_path: row.get(2)?,
                    branch_name: row.get(3)?,
                    age_minutes: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(ListOutput {
            total: claims.len(),
            claims,
        })
    })
}
