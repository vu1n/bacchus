//! Next task tool - gets ready task, creates worktree, claims it
//!
//! Combines task querying, worktree creation, and claiming in one operation.
//! Uses atomic SQLite claiming for task management.

use crate::tasks;
use crate::worktree;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct NextOutput {
    pub success: bool,
    pub task_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub message: String,
}

pub fn next_task(agent_id: &str, workspace_root: &Path) -> Result<NextOutput> {
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

            Ok(NextOutput {
                success: true,
                task_id: Some(task.id.clone()),
                title: Some(task.title),
                description: task.description,
                worktree_path: Some(wt.path.to_string_lossy().to_string()),
                branch: Some(wt.branch),
                message: format!("Claimed {} - work in {}", task.id, wt.path.display()),
            })
        }
        Ok(None) => Ok(NextOutput {
            success: false,
            task_id: None,
            title: None,
            description: None,
            worktree_path: None,
            branch: None,
            message: "No ready tasks available".to_string(),
        }),
        Err(e) => Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to query tasks: {}", e)),
        )),
    }
}
