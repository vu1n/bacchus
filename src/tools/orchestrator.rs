//! Orchestrator release loop - merges tasks marked ready_for_release.
//!
//! This is the state-machine bridge between agent completion and integration:
//! ready_for_release -> releasing -> closed / needs_resolution.

use crate::events;
use crate::quality;
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
    pub dedup_tasks_created: usize,
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
        dedup_tasks_created: 0,
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

                // Detect duplicate symbols post-merge (non-blocking)
                let dedup_count = detect_and_create_dedup_tasks(
                    workspace_root,
                    &task,
                    run_id,
                );
                output.dedup_tasks_created += dedup_count;

                // Re-index changed files so symbols table reflects merged state
                reindex_workspace_changes(workspace_root, &task.id);

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
                    &serde_json::json!({ "commit_id": commit_id, "dedup_tasks_created": dedup_count }),
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

/// Detect duplicate symbols between the task's changed files and the existing index,
/// and auto-create cleanup tasks for any duplicates found.
///
/// Returns the number of dedup tasks created.
fn detect_and_create_dedup_tasks(
    workspace_root: &Path,
    task: &tasks::SqliteTask,
    run_id: Option<&str>,
) -> usize {
    // Get list of changed files from workspace via jj diff
    let ws_path = workspace::get_workspaces_dir(workspace_root).join(&task.id);
    let changed_files = match get_changed_files(&ws_path) {
        Ok(files) => files,
        Err(_) => return 0,
    };

    if changed_files.is_empty() {
        return 0;
    }

    let duplicates = quality::detect_duplicate_symbols(&changed_files);
    if duplicates.is_empty() {
        return 0;
    }

    // Build description listing duplicate pairs
    let mut desc = String::from("Duplicate symbols detected after merge. Consolidate:\n\n");
    let mut dedup_files = std::collections::HashSet::new();
    for dup in &duplicates {
        desc.push_str(&format!(
            "- `{}` in `{}` duplicates `{}` in `{}` (hash: {})\n",
            dup.new_symbol, dup.new_file, dup.existing_symbol, dup.existing_file, dup.hash
        ));
        dedup_files.insert(dup.new_file.clone());
        dedup_files.insert(dup.existing_file.clone());
    }

    // Create a single cleanup task for all duplicates from this merge
    let short_hash = &task.id[..task.id.len().min(8)];
    let dedup_id = format!("{}-DEDUP-{}", task.epic_id, short_hash);

    let footprint = tasks::TaskFootprint {
        modifies: dedup_files.into_iter().collect(),
        creates: Vec::new(),
    };

    let input = tasks::CreateSqliteTaskInput {
        id: dedup_id.clone(),
        epic_id: task.epic_id.clone(),
        title: format!("Consolidate duplicate symbols from {}", task.id),
        description: Some(desc),
        priority: 8, // lower priority than normal tasks
        depends_on: vec![task.id.clone()],
        task_type: Some(tasks::SqliteTaskType::Refactor),
        archetype: Some(task.archetype.clone()),
        footprint,
    };

    match tasks::create_sqlite_task(input) {
        Ok(_) => {
            let _ = events::record_event(
                run_id,
                "orchestrator",
                "duplicate_symbols_detected",
                "task",
                &task.id,
                &serde_json::json!({
                    "duplicate_count": duplicates.len(),
                    "dedup_task_id": dedup_id,
                }),
                Some(&format!("dedup-task-created:{}:{}", task.id, dedup_id)),
            );
            1
        }
        Err(tasks::TasksError::DuplicateTask(_)) => 0, // already created
        Err(e) => {
            eprintln!("Warning: failed to create dedup task {}: {}", dedup_id, e);
            0
        }
    }
}

/// Get list of changed files from a workspace using jj diff --stat.
fn get_changed_files(ws_path: &Path) -> Result<Vec<String>, String> {
    if !ws_path.exists() {
        return Ok(Vec::new());
    }

    let output = std::process::Command::new("jj")
        .args([
            "-R",
            ws_path.to_str().unwrap_or("."),
            "diff",
            "--stat",
        ])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            // jj diff --stat format: "file.rs | N +++---"
            let trimmed = line.trim();
            if trimmed.contains('|') {
                Some(
                    trimmed
                        .split('|')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                )
            } else {
                None
            }
        })
        .filter(|f| !f.is_empty())
        .collect();

    Ok(files)
}

/// Re-index changed files from a workspace so the symbols table reflects merged state.
fn reindex_workspace_changes(workspace_root: &Path, task_id: &str) {
    let ws_path = workspace::get_workspaces_dir(workspace_root).join(task_id);
    let files = match get_changed_files(&ws_path) {
        Ok(f) => f,
        Err(_) => return,
    };

    for file in &files {
        let file_path = workspace_root.join(file);
        if file_path.is_file() {
            let mut parser = match crate::indexer::Parser::new() {
                Ok(p) => p,
                Err(_) => return,
            };
            if let Ok(symbols) = crate::parse_file(&mut parser, &file_path, &workspace_root.to_path_buf()) {
                let _ = crate::store_symbols(&symbols);
            }
        }
    }
}
