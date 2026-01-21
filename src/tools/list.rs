//! List active claims and jj workspaces
//!
//! Uses SQLite-based task management.

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
    pub task_id: String,
    pub agent_id: String,
    pub workspace_path: String,
    pub age_minutes: i64,
}

/// List all active claims from SQLite
pub fn list_claims() -> Result<ListOutput> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, claimed_by, claimed_at
             FROM tasks
             WHERE status = 'in_progress'
               AND claimed_by IS NOT NULL
               AND deleted_at IS NULL
             ORDER BY claimed_at DESC",
        )?;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let claims: Vec<ClaimInfo> = stmt
            .query_map([], |row| {
                let task_id: String = row.get(0)?;
                let claimed_at: Option<i64> = row.get(2)?;
                let age_minutes = claimed_at.map(|ca| (now_ms - ca) / 60000).unwrap_or(0);

                Ok(ClaimInfo {
                    task_id: task_id.clone(),
                    agent_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    workspace_path: format!(".bacchus/workspaces/{}", task_id),
                    age_minutes,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        // Sort by age (most recent first)
        let mut sorted_claims = claims;
        sorted_claims.sort_by(|a, b| a.age_minutes.cmp(&b.age_minutes));

        Ok(ListOutput {
            total: sorted_claims.len(),
            claims: sorted_claims,
        })
    })
}
