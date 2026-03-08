//! Tool implementations for Bacchus
//!
//! Each tool corresponds to a CLI command.

pub mod abort;
pub mod archetypes;
pub mod claim;
pub mod context;
pub mod eval;
pub mod init;
pub mod list;
pub mod next;
pub mod orchestrator;
pub mod release;
pub mod resolve;
pub mod review;
pub mod session;
pub mod stale;
pub mod symbols;
pub mod task_commands;

/// Unified error type for tool-layer operations.
///
/// Replaces `Box<dyn Error>` in claim/release/resolve/next, making error
/// provenance traceable without string-parsing.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Task(#[from] crate::tasks::TasksError),

    #[error("{0}")]
    Workspace(#[from] crate::workspace::WorkspaceError),

    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("{0}")]
    Other(String),
}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        ToolError::Other(s)
    }
}

/// Look up a task and verify it has an active claim.
///
/// Returns `Ok((task, agent_id))` on success, or an `Ok(Err(message))` with the
/// user-facing failure reason when the task is missing / unclaimed.
pub fn require_claimed_task(
    task_id: &str,
) -> Result<Result<(crate::tasks::SqliteTask, String), String>, ToolError> {
    match crate::tasks::get_sqlite_task(task_id) {
        Ok(task) => {
            if task.claimed_by.is_none() {
                Ok(Err(format!("No claim found for {}", task_id)))
            } else {
                let agent_id = task.claimed_by.clone().unwrap_or_default();
                Ok(Ok((task, agent_id)))
            }
        }
        Err(crate::tasks::TasksError::TaskNotFound(_)) => {
            Ok(Err(format!("Task {} not found", task_id)))
        }
        Err(e) => Err(e.into()),
    }
}

pub use abort::abort_merge;
pub use archetypes::{
    cmd_archetype_prompt, cmd_list_archetypes, cmd_select_archetype, cmd_show_archetype,
};
pub use claim::claim_task;
pub use context::generate_context;
pub use eval::generate_eval_report;
pub use init::{init_workspace, update_assets, InitOptions};
pub use list::list_claims;
pub use next::next_task;
pub use orchestrator::{process_ready_releases, verify_release_invariants};
pub use release::{release_task, ReleaseStatus};
pub use resolve::resolve_merge;
pub use review::review_task;
pub use session::{
    check_session, prune_sessions, run_agent_heartbeat_loop, run_orchestrator_lease_loop,
    run_worker_command, session_status, spawn_workers_once, start_session, stop_session,
    SessionMode,
};
pub use stale::find_stale;
pub use symbols::{find_symbols, find_symbols_handle, FindSymbolsInput};
pub use task_commands::{import_tasks, init_tasks, list_tasks, show_task, validate_tasks};
