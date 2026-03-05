//! Release task tool - marks task ready for release or discards workspace
//!
//! In jj workflow, agents don't merge directly. Instead:
//! - "done": Validates single-commit, marks task ready_for_release
//! - "blocked": Keeps workspace, marks as blocked
//! - "failed": Removes workspace, resets to open
//!
//! The orchestrator handles actual release (rebase onto main, advance bookmark).

use crate::quality::{self, QualityCheck};
use crate::tasks::{self, SqliteTaskStatus};
use crate::tools::eval::{self, EventType};
use crate::workspace;
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::ToolError;

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum ReleaseStatus {
    /// Mark task ready for orchestrator merge
    Done,
    /// Keep workspace, mark as blocked
    Blocked,
    /// Remove workspace, reset to open
    Failed,
}

impl std::fmt::Display for ReleaseStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReleaseStatus::Done => write!(f, "done"),
            ReleaseStatus::Blocked => write!(f, "blocked"),
            ReleaseStatus::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseOutput {
    pub success: bool,
    pub task_id: String,
    pub status: String,
    pub ready_for_release: bool,
    pub commit_id: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quality_checks: Vec<QualityCheck>,
}

impl ReleaseOutput {
    fn failure(task_id: &str, status: &str, commit_id: Option<String>, message: String) -> Self {
        Self {
            success: false,
            task_id: task_id.to_string(),
            status: status.to_string(),
            ready_for_release: false,
            commit_id,
            message,
            quality_checks: Vec::new(),
        }
    }
}

pub fn release_task(
    task_id: &str,
    status: ReleaseStatus,
    workspace_root: &Path,
) -> Result<ReleaseOutput, ToolError> {
    let (_task, agent_id) = match super::require_claimed_task(task_id)? {
        Ok(pair) => pair,
        Err(msg) => {
            return Ok(ReleaseOutput::failure(
                task_id,
                &status.to_string(),
                None,
                msg,
            ));
        }
    };

    let status_str = status.to_string();

    match status {
        ReleaseStatus::Done => {
            // Validate single-commit workflow before marking ready
            let commit_id = match workspace::validate_single_commit(workspace_root, task_id) {
                Ok(id) => id,
                Err(workspace::WorkspaceError::NoCommits(..)) => {
                    return Ok(ReleaseOutput::failure(
                        task_id,
                        &status_str,
                        None,
                        format!(
                            "Task {} has no commits. Make changes before marking done.",
                            task_id
                        ),
                    ));
                }
                Err(workspace::WorkspaceError::MultipleCommits(_, count)) => {
                    return Ok(ReleaseOutput::failure(
                        task_id,
                        &status_str,
                        None,
                        format!(
                            "Task {} has {} commits. Squash to single commit before marking done.",
                            task_id, count
                        ),
                    ));
                }
                Err(e) => {
                    return Ok(ReleaseOutput::failure(
                        task_id,
                        &status_str,
                        None,
                        format!("Failed to validate workspace: {}", e),
                    ));
                }
            };

            // Check for conflicts before marking ready
            if workspace::has_conflicts(workspace_root, task_id).unwrap_or(false) {
                let files =
                    workspace::get_conflict_files(workspace_root, task_id).unwrap_or_default();
                return Ok(ReleaseOutput::failure(
                    task_id,
                    &status_str,
                    None,
                    format!(
                        "Task {} has conflicts in: {}. Resolve before marking done.",
                        task_id,
                        files.join(", ")
                    ),
                ));
            }

            // Run quality gate if configured
            if let Some(config) = quality::load_config(workspace_root) {
                let ws_path = workspace::get_workspaces_dir(workspace_root).join(task_id);
                match quality::run_quality_gate(&config, &ws_path) {
                    Ok(gate) if !gate.passed => {
                        quality::store_quality_results(task_id, &gate.checks);
                        return Ok(ReleaseOutput {
                            success: false,
                            task_id: task_id.to_string(),
                            status: "quality_gate_failed".to_string(),
                            ready_for_release: false,
                            commit_id: Some(commit_id),
                            message: quality::format_gate_failures(&gate),
                            quality_checks: gate.checks,
                        });
                    }
                    Ok(gate) => {
                        quality::store_quality_results(task_id, &gate.checks);
                    }
                    Err(e) => {
                        // Gate runner error — fail-open with warning, don't block release
                        eprintln!("Warning: quality gate runner failed for {}: {}", task_id, e);
                    }
                }
            }

            // Mark task ready for release (orchestrator will handle actual merge)
            tasks::mark_task_ready_for_release(task_id, &agent_id, &commit_id)?;

            // Record eval event
            let _ = eval::record_event(task_id, &agent_id, EventType::Completed, None);

            Ok(ReleaseOutput {
                success: true,
                task_id: task_id.to_string(),
                status: "ready_for_release".to_string(),
                ready_for_release: true,
                commit_id: Some(commit_id),
                message: format!(
                    "Task {} marked ready for release. Orchestrator will merge.",
                    task_id
                ),
                quality_checks: Vec::new(),
            })
        }
        ReleaseStatus::Blocked => {
            // Keep workspace, mark as blocked
            tasks::reset_sqlite_task(task_id, SqliteTaskStatus::Blocked)?;

            // Record eval event
            let _ = eval::record_event(task_id, &agent_id, EventType::Blocked, None);

            Ok(ReleaseOutput {
                success: true,
                task_id: task_id.to_string(),
                status: status_str,
                ready_for_release: false,
                commit_id: None,
                message: format!("Task {} marked as blocked. Workspace preserved.", task_id),
                quality_checks: Vec::new(),
            })
        }
        ReleaseStatus::Failed => {
            // Remove workspace, reset to open
            let _ = workspace::remove_workspace(workspace_root, task_id);
            tasks::reset_sqlite_task(task_id, SqliteTaskStatus::Open)?;

            // Record eval event
            let _ = eval::record_event(task_id, &agent_id, EventType::Failed, None);

            Ok(ReleaseOutput {
                success: true,
                task_id: task_id.to_string(),
                status: status_str,
                ready_for_release: false,
                commit_id: None,
                message: format!(
                    "Task {} failed. Workspace removed, task reset to open.",
                    task_id
                ),
                quality_checks: Vec::new(),
            })
        }
    }
}
