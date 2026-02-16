//! Next task tool - gets ready task, creates jj workspace, claims it
//!
//! Combines task querying, workspace creation, and claiming in one operation.
//! Uses atomic SQLite claiming for task management.

use crate::tasks;
use crate::tools::session;
use crate::workspace;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct NextOutput {
    pub success: bool,
    pub task_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub workspace_path: Option<String>,
    pub message: String,
}

pub fn next_task(agent_id: &str, workspace_root: &Path) -> Result<NextOutput> {
    // Atomic claim: claim_next_sqlite_task handles readiness check and claim in one transaction
    match tasks::claim_next_sqlite_task(agent_id) {
        Ok(Some(task)) => {
            // Create jj workspace for the claimed task
            let ws = match workspace::create_workspace(workspace_root, &task.id) {
                Ok(ws) => ws,
                Err(e) => {
                    // Rollback: return task to open state if workspace creation fails.
                    let _ = tasks::reset_sqlite_task(&task.id, tasks::SqliteTaskStatus::Open);
                    return Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(format!("Failed to create workspace: {}", e)),
                    ));
                }
            };

            Ok(NextOutput {
                success: true,
                task_id: Some(task.id.clone()),
                title: Some(task.title),
                description: task.description,
                workspace_path: Some(ws.path.to_string_lossy().to_string()),
                message: {
                    let mut message =
                        format!("Claimed {} - work in {}", task.id, ws.path.display());
                    if let Err(e) = session::attach_agent_session_heartbeat(&task.id, agent_id) {
                        message = format!("{} (heartbeat loop unavailable: {})", message, e);
                    }
                    message
                },
            })
        }
        Ok(None) => Ok(NextOutput {
            success: false,
            task_id: None,
            title: None,
            description: None,
            workspace_path: None,
            message: "No ready tasks available".to_string(),
        }),
        Err(e) => Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some(format!("Failed to query tasks: {}", e)),
        )),
    }
}
