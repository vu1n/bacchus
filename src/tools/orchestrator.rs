//! Orchestrator release loop - merges tasks marked ready_for_release.
//!
//! This is the state-machine bridge between agent completion and integration:
//! ready_for_release -> releasing -> closed / needs_resolution.

use crate::events;
use crate::tasks::{self, SqliteTaskStatus};
use crate::workspace::{self, ReleaseResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

const RELEASE_LEASE_TIMEOUT_MS: i64 = 10 * 60 * 1000;

fn renew_orchestrator_lease(run_id: Option<&str>) -> Result<(), String> {
    let Some(run_id) = run_id else {
        return Ok(());
    };

    match tasks::try_acquire_orchestrator_lease(run_id, tasks::ORCHESTRATOR_LEASE_TTL_MS) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "Lost orchestrator leader lease while processing releases (run_id={}).",
            run_id
        )),
        Err(e) => Err(format!("Failed to renew orchestrator leader lease: {}", e)),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseTaskResult {
    pub task_id: String,
    pub status: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflict_files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessReleasesOutput {
    pub success: bool,
    pub processed: usize,
    pub reconciled: usize,
    pub merged: usize,
    pub conflicts: usize,
    pub failed: usize,
    pub results: Vec<ReleaseTaskResult>,
}

/// Process tasks currently marked `ready_for_release`.
///
/// The orchestrator performs:
/// 1. status transition `ready_for_release -> releasing`
/// 2. rebase task workspace onto `main`
/// 3. on success, advance `main`, close task, cleanup workspace
/// 4. on conflicts, mark `needs_resolution`
pub fn process_ready_releases(
    workspace_root: &Path,
    limit: Option<usize>,
    run_id: Option<&str>,
) -> Result<ProcessReleasesOutput, String> {
    renew_orchestrator_lease(run_id)?;

    let now = chrono::Utc::now().timestamp_millis();
    let lease_cutoff = now - RELEASE_LEASE_TIMEOUT_MS;

    let mut output = ProcessReleasesOutput {
        success: true,
        processed: 0,
        reconciled: 0,
        merged: 0,
        conflicts: 0,
        failed: 0,
        results: Vec::new(),
    };

    // First reconcile interrupted releases.
    let releasing_tasks = tasks::list_sqlite_tasks(None, Some(SqliteTaskStatus::Releasing), false)
        .map_err(|e| e.to_string())?;
    for task in releasing_tasks {
        renew_orchestrator_lease(run_id)?;

        if let Some(commit_id) = task.release_commit_id.as_deref() {
            let commit_in_main = match workspace::is_commit_in_main(workspace_root, commit_id) {
                Ok(v) => v,
                Err(e) => {
                    output.failed += 1;
                    output.results.push(ReleaseTaskResult {
                        task_id: task.id.clone(),
                        status: "failed".to_string(),
                        message: format!("Failed to verify commit on main: {}", e),
                        commit_id: Some(commit_id.to_string()),
                        conflict_files: Vec::new(),
                    });
                    continue;
                }
            };

            if commit_in_main {
                if let Err(e) = tasks::complete_task_release(&task.id) {
                    output.failed += 1;
                    output.results.push(ReleaseTaskResult {
                        task_id: task.id.clone(),
                        status: "failed".to_string(),
                        message: format!("Reconcile failed closing task: {}", e),
                        commit_id: Some(commit_id.to_string()),
                        conflict_files: Vec::new(),
                    });
                    continue;
                }

                let _ = workspace::complete_release(workspace_root, &task.id);
                let _ = events::record_event(
                    run_id,
                    "orchestrator",
                    "release_reconciled",
                    "task",
                    &task.id,
                    &serde_json::json!({ "commit_id": commit_id }),
                    Some(&format!("release-reconciled:{}:{}", task.id, commit_id)),
                );

                output.reconciled += 1;
                output.merged += 1;
                output.results.push(ReleaseTaskResult {
                    task_id: task.id,
                    status: "closed".to_string(),
                    message: "Recovered previously merged release and closed task.".to_string(),
                    commit_id: Some(commit_id.to_string()),
                    conflict_files: Vec::new(),
                });
                continue;
            }
        }

        if task.release_started_at.unwrap_or(0) < lease_cutoff {
            if let Err(e) = tasks::reset_task_release_to_ready(&task.id) {
                output.failed += 1;
                output.results.push(ReleaseTaskResult {
                    task_id: task.id.clone(),
                    status: "failed".to_string(),
                    message: format!("Release lease expired but reset failed: {}", e),
                    commit_id: task.release_commit_id.clone(),
                    conflict_files: Vec::new(),
                });
            } else {
                let _ = events::record_event(
                    run_id,
                    "orchestrator",
                    "release_reset_timeout",
                    "task",
                    &task.id,
                    &serde_json::json!({ "lease_timeout_ms": RELEASE_LEASE_TIMEOUT_MS }),
                    Some(&format!("release-timeout-reset:{}", task.id)),
                );
                output.reconciled += 1;
                output.results.push(ReleaseTaskResult {
                    task_id: task.id,
                    status: "ready_for_release".to_string(),
                    message: "Release lease expired; task reset to ready_for_release.".to_string(),
                    commit_id: None,
                    conflict_files: Vec::new(),
                });
            }
        }
    }

    let ready_tasks = tasks::get_tasks_ready_for_release().map_err(|e| e.to_string())?;
    let max = limit.unwrap_or(usize::MAX);

    for task in ready_tasks.into_iter().take(max) {
        renew_orchestrator_lease(run_id)?;

        let task_id = task.id.clone();
        output.processed += 1;

        let ready_commit = task.ready_commit_id.clone().unwrap_or_default();
        let _ = events::record_event(
            run_id,
            "orchestrator",
            "release_start",
            "task",
            &task.id,
            &serde_json::json!({ "ready_commit_id": ready_commit }),
            Some(&format!("release-start:{}:{}", task.id, ready_commit)),
        );

        if let Err(e) = tasks::start_task_release(&task_id) {
            output.failed += 1;
            let _ = events::record_event(
                run_id,
                "orchestrator",
                "release_failed",
                "task",
                &task_id,
                &serde_json::json!({ "stage": "start", "error": e.to_string() }),
                None,
            );
            output.results.push(ReleaseTaskResult {
                task_id,
                status: "failed".to_string(),
                message: format!("Failed to start release: {}", e),
                commit_id: None,
                conflict_files: Vec::new(),
            });
            continue;
        }

        match workspace::rebase_workspace_onto_main(workspace_root, &task.id) {
            Ok(ReleaseResult::Conflicts { files }) => {
                let _ = tasks::mark_task_needs_resolution(&task.id, &files);
                let _ = events::record_event(
                    run_id,
                    "orchestrator",
                    "release_conflict",
                    "task",
                    &task.id,
                    &serde_json::json!({ "files": files }),
                    None,
                );
                output.conflicts += 1;
                output.results.push(ReleaseTaskResult {
                    task_id: task.id,
                    status: "needs_resolution".to_string(),
                    message: "Rebase produced conflicts. Resolve and re-release.".to_string(),
                    commit_id: None,
                    conflict_files: files,
                });
            }
            Ok(ReleaseResult::Success { commit_id }) => {
                if let Err(e) = tasks::set_task_release_commit(&task.id, &commit_id) {
                    let _ = tasks::reset_task_release_to_ready(&task.id);
                    let _ = events::record_event(
                        run_id,
                        "orchestrator",
                        "release_failed",
                        "task",
                        &task.id,
                        &serde_json::json!({ "stage": "set_release_commit", "error": e.to_string() }),
                        None,
                    );
                    output.failed += 1;
                    output.results.push(ReleaseTaskResult {
                        task_id: task.id,
                        status: "failed".to_string(),
                        message: format!("Failed to record release commit: {}", e),
                        commit_id: None,
                        conflict_files: Vec::new(),
                    });
                    continue;
                }

                if let Err(e) = workspace::advance_main_bookmark(workspace_root, &commit_id) {
                    let _ = tasks::reset_task_release_to_ready(&task.id);
                    let _ = events::record_event(
                        run_id,
                        "orchestrator",
                        "release_failed",
                        "task",
                        &task.id,
                        &serde_json::json!({ "stage": "advance_main", "error": e.to_string() }),
                        None,
                    );
                    output.failed += 1;
                    output.results.push(ReleaseTaskResult {
                        task_id: task.id,
                        status: "failed".to_string(),
                        message: format!("Failed to advance main bookmark: {}", e),
                        commit_id: Some(commit_id),
                        conflict_files: Vec::new(),
                    });
                    continue;
                }

                if let Err(e) = tasks::complete_task_release(&task.id) {
                    let _ = events::record_event(
                        run_id,
                        "orchestrator",
                        "release_failed",
                        "task",
                        &task.id,
                        &serde_json::json!({ "stage": "complete_task_release", "error": e.to_string(), "commit_id": commit_id }),
                        None,
                    );
                    output.failed += 1;
                    output.results.push(ReleaseTaskResult {
                        task_id: task.id,
                        status: "failed".to_string(),
                        message: format!("Merged to main but failed to close task in DB: {}", e),
                        commit_id: Some(commit_id),
                        conflict_files: Vec::new(),
                    });
                    continue;
                }

                let cleanup_message = match workspace::complete_release(workspace_root, &task.id) {
                    Ok(_) => "Merged and cleaned workspace".to_string(),
                    Err(e) => format!("Merged but cleanup warning: {}", e),
                };

                let _ = events::record_event(
                    run_id,
                    "orchestrator",
                    "release_merged",
                    "task",
                    &task.id,
                    &serde_json::json!({ "commit_id": commit_id }),
                    Some(&format!("release-merged:{}:{}", task.id, commit_id)),
                );

                output.merged += 1;
                output.results.push(ReleaseTaskResult {
                    task_id: task.id,
                    status: "closed".to_string(),
                    message: cleanup_message,
                    commit_id: Some(commit_id),
                    conflict_files: Vec::new(),
                });
            }
            Err(e) => {
                let _ = tasks::reset_task_release_to_ready(&task.id);
                let _ = events::record_event(
                    run_id,
                    "orchestrator",
                    "release_failed",
                    "task",
                    &task.id,
                    &serde_json::json!({ "stage": "rebase", "error": e.to_string() }),
                    None,
                );
                output.failed += 1;
                output.results.push(ReleaseTaskResult {
                    task_id: task.id,
                    status: "failed".to_string(),
                    message: format!("Release attempt failed: {}", e),
                    commit_id: None,
                    conflict_files: Vec::new(),
                });
            }
        }
    }

    output.success = output.failed == 0;
    Ok(output)
}
