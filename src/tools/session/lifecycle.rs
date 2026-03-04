//! Session lifecycle: start, stop, status, and prune operations.

use crate::config::{current_session_scope_id, find_workspace_root};
use crate::handles;
use crate::quality;
use crate::tasks;
use crate::workers;
use std::fs;
use std::io::ErrorKind;

use super::config::*;
use super::file::*;
use super::heartbeat::{attach_agent_session_heartbeat, spawn_orchestrator_lease_loop};
use super::types::{Session, SessionMode};
use super::workers as session_workers;

/// Start a session
pub fn start_session(
    mode: SessionMode,
    task_id: Option<&str>,
    max_concurrent: i32,
    agent_id: Option<&str>,
    epic_id: Option<&str>,
    goal: Option<&str>,
) -> Result<String, String> {
    let root = find_workspace_root().ok_or("No workspace root found")?;
    let bacchus_dir = root.join(".bacchus");
    fs::create_dir_all(&bacchus_dir).map_err(|e| e.to_string())?;
    let session_file = scoped_session_path().ok_or("No workspace root found")?;
    if let Some(parent) = session_file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let session = match mode {
        SessionMode::Agent => {
            let task_id = task_id.ok_or("task_id required for agent mode")?;
            let claim_owner = tasks::get_sqlite_task(task_id)
                .ok()
                .and_then(|task| task.claimed_by);
            Session {
                mode: SessionMode::Agent,
                task_id: Some(task_id.to_string()),
                max_concurrent: None,
                agent_id: agent_id.map(String::from).or(claim_owner),
                run_id: Some(session_workers::generate_run_id("agent")),
                agent_heartbeat_token: None,
                orchestrator_lease_token: None,
                started_at: chrono::Utc::now().to_rfc3339(),
            }
        }
        SessionMode::Orchestrator => {
            let run_id = session_workers::generate_run_id("orchestrator");
            let acquired = tasks::try_acquire_orchestrator_lease(
                &run_id,
                configured_orchestrator_lease_ttl_ms(),
            )
            .map_err(|e| e.to_string())?;
            if !acquired {
                let details = session_workers::describe_existing_orchestrator_lease()
                    .unwrap_or_else(|| "holder=unknown".to_string());
                return Err(format!(
                    "Another orchestrator leader lease is active ({}).",
                    details
                ));
            }

            Session {
                mode: SessionMode::Orchestrator,
                task_id: None,
                max_concurrent: Some(max_concurrent),
                agent_id: None,
                run_id: Some(run_id),
                agent_heartbeat_token: None,
                orchestrator_lease_token: Some(session_workers::generate_run_id("lease")),
                started_at: chrono::Utc::now().to_rfc3339(),
            }
        }
        SessionMode::Architect => {
            let agent_id = agent_id.ok_or("agent_id required for architect mode")?;
            Session {
                mode: SessionMode::Architect,
                task_id: None,
                max_concurrent: None,
                agent_id: Some(agent_id.to_string()),
                run_id: Some(session_workers::generate_run_id("architect")),
                agent_heartbeat_token: None,
                orchestrator_lease_token: None,
                started_at: chrono::Utc::now().to_rfc3339(),
            }
        }
    };

    if current_session_scope_id() == "default" && session_file.exists() {
        if let Ok(existing) = read_session(&session_file) {
            if !same_default_scope_identity(&existing, &session) {
                return Err(
                    "Default session scope is already occupied. Set BACCHUS_SESSION_ID to run concurrent sessions."
                        .to_string(),
                );
            }
        }
    }

    if let Err(e) = write_session(&session_file, &session) {
        if session.mode == SessionMode::Orchestrator {
            if let Some(run_id) = session.run_id.as_deref() {
                let _ = tasks::release_orchestrator_lease(run_id);
            }
        }
        return Err(e);
    }
    // Keep legacy path in sync for default scope during migration.
    if current_session_scope_id() == "default" {
        if let Some(legacy) = legacy_session_path() {
            let _ = write_session(&legacy, &session);
        }
    }

    let mut message = format!("Started {} session", mode);
    if matches!(mode, SessionMode::Agent) {
        if let (Some(task), Some(owner)) = (session.task_id.as_deref(), session.agent_id.as_deref())
        {
            if let Err(e) = attach_agent_session_heartbeat(task, owner) {
                message = format!("{} (heartbeat loop unavailable: {})", message, e);
            }
        }
    } else if matches!(mode, SessionMode::Orchestrator) {
        if let (Some(run_id), Some(token)) = (
            session.run_id.as_deref(),
            session.orchestrator_lease_token.as_deref(),
        ) {
            if let Err(e) = spawn_orchestrator_lease_loop(
                run_id,
                token,
                configured_orchestrator_lease_interval_ms(),
            ) {
                message = format!("{} (lease loop unavailable: {})", message, e);
            }
        }
        // Write orchestrator breadcrumb so protocol survives context compaction
        write_orchestrator_breadcrumb(&root, &session, epic_id, goal);
    }

    Ok(message)
}

/// Stop the session and clean up session-scoped handles
pub fn stop_session() -> Result<String, String> {
    if let Some(path) = session_path().filter(|p| p.exists()) {
        let session = read_session(&path).ok();

        if let Some(s) = session.as_ref() {
            if s.mode == SessionMode::Orchestrator {
                if let Some(run_id) = s.run_id.as_deref() {
                    let _ = tasks::release_orchestrator_lease(run_id);
                    let _ = workers::fail_active_workers(run_id, "orchestrator session stopped");
                    if let Some(root) = find_workspace_root() {
                        let _ = session_workers::reconcile_failed_worker_tasks(&root, run_id);
                    }
                }

                // Run desloppify mechanical scan if configured and available (non-blocking)
                if let Some(root) = find_workspace_root() {
                    let scan = quality::run_desloppify_scan(&root);
                    if scan.ran {
                        if let Some(report) = &scan.report_path {
                            eprintln!(
                                "Desloppify scan: {} findings (report: {})",
                                scan.findings_count, report
                            );
                        }
                        // Create cleanup tasks for findings
                        if scan.findings_count > 0 {
                            create_desloppify_cleanup_tasks(&root, &scan);
                        }
                    }

                    // Re-index the project so the symbol table is fresh for the next session.
                    // process-releases re-indexes per-merge, but drift can accumulate from
                    // manual fixes, desloppify cleanup, or partial indexing failures.
                    match crate::index_path(".", &root) {
                        Ok(n) => eprintln!("Re-indexed {} files at session end", n),
                        Err(e) => eprintln!("Warning: re-index at session end failed: {}", e),
                    }
                }

                // Remove orchestrator breadcrumb
                if let Some(root) = find_workspace_root() {
                    let breadcrumb = root.join(".bacchus/ORCHESTRATOR.md");
                    let _ = fs::remove_file(&breadcrumb);
                }
            }
        }

        // Remove session file
        match fs::remove_file(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        // Remove legacy default scope file if present.
        if current_session_scope_id() == "default" {
            if let Some(legacy) = legacy_session_path() {
                if legacy != path {
                    let _ = fs::remove_file(&legacy);
                }
            }
        }

        // Clear handles for this session
        let handles_cleared = if let Some(sid) = session.as_ref().map(|s| s.started_at.clone()) {
            handles::clear_session_handles(&sid).unwrap_or(0)
        } else {
            0
        };

        if handles_cleared > 0 {
            return Ok(format!(
                "Session stopped. Cleared {} handle(s).",
                handles_cleared
            ));
        }
        return Ok("Session stopped".to_string());
    }
    Ok("No active session".to_string())
}

/// Get current session status
pub fn session_status() -> Result<serde_json::Value, String> {
    if let Some(path) = session_path().filter(|p| p.exists()) {
        let session = read_session(&path)?;
        let mut base = serde_json::json!({
            "active": true,
            "session": session,
            "scope_id": current_session_scope_id(),
            "path": path.to_string_lossy()
        });

        if let Some(run_id) = base["session"]["run_id"].as_str() {
            if base["session"]["mode"].as_str() == Some("orchestrator") {
                let worker_stats =
                    workers::get_run_worker_stats(run_id).unwrap_or(workers::WorkerRunStats {
                        launching: 0,
                        running: 0,
                        completed: 0,
                        failed: 0,
                    });
                let active_workers = workers::count_active_workers(run_id).unwrap_or(0);
                let reopenable_failed_tasks = workers::list_reopenable_failed_worker_tasks(run_id)
                    .map(|v| v.len())
                    .unwrap_or(0);
                let ready_count = tasks::get_ready_sqlite_tasks(None)
                    .map(|v| v.len())
                    .unwrap_or(0);
                base["orchestrator"] = serde_json::json!({
                    "run_id": run_id,
                    "active_workers": active_workers,
                    "worker_stats": worker_stats,
                    "ready_tasks": ready_count,
                    "reopenable_failed_tasks": reopenable_failed_tasks
                });
            }
        }
        return Ok(base);
    }
    Ok(serde_json::json!({
        "active": false,
        "scope_id": current_session_scope_id(),
        "session": null
    }))
}

/// Remove stale scoped session files and clean expired orphaned leader leases.
pub fn prune_sessions(minutes: i64) -> Result<serde_json::Value, String> {
    let threshold = minutes.max(1);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let current_scope = current_session_scope_id();
    let mut removed_scopes: Vec<String> = Vec::new();
    let mut kept_scopes: Vec<String> = Vec::new();
    let mut released_leases: Vec<String> = Vec::new();

    if let Some(dir) = sessions_dir() {
        if dir.exists() {
            let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let scope = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if scope == current_scope {
                    kept_scopes.push(scope);
                    continue;
                }

                let session = match read_session(&path) {
                    Ok(s) => s,
                    Err(_) => {
                        // Best effort cleanup for malformed stale files.
                        let _ = fs::remove_file(&path);
                        removed_scopes.push(scope);
                        continue;
                    }
                };

                let age = session_age_minutes(&session, &path).unwrap_or(0);
                if age >= threshold {
                    if session.mode == SessionMode::Orchestrator {
                        if let Some(run_id) = session.run_id.as_deref() {
                            let _ = tasks::release_orchestrator_lease(run_id);
                            released_leases.push(run_id.to_string());
                        }
                    }
                    let _ = fs::remove_file(&path);
                    removed_scopes.push(scope);
                } else {
                    kept_scopes.push(scope);
                }
            }
        }
    }

    // Cleanup an expired orphaned lease not referenced by any remaining session file.
    let mut active_run_ids = std::collections::HashSet::new();
    if let Some(dir) = sessions_dir() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if let Ok(session) = read_session(&path) {
                    if session.mode == SessionMode::Orchestrator {
                        if let Some(run_id) = session.run_id {
                            active_run_ids.insert(run_id);
                        }
                    }
                }
            }
        }
    }
    if let Ok(Some(lease)) = tasks::get_orchestrator_lease() {
        if lease.lease_expires_at < now_ms
            && !active_run_ids.contains(&lease.holder_id)
            && tasks::release_orchestrator_lease(&lease.holder_id).is_ok()
        {
            released_leases.push(lease.holder_id);
        }
    }

    Ok(serde_json::json!({
        "success": true,
        "scope_id": current_scope,
        "threshold_minutes": threshold,
        "removed_scopes": removed_scopes,
        "kept_scopes": kept_scopes,
        "released_orchestrator_leases": released_leases
    }))
}

/// Create cleanup tasks from desloppify scan findings.
///
/// Non-blocking: errors are logged but don't affect session stop.
fn create_desloppify_cleanup_tasks(
    workspace_root: &std::path::Path,
    scan: &quality::DesloppifyScanResult,
) {
    // Try to read the report for details
    let report_path = match &scan.report_path {
        Some(p) => p.clone(),
        None => return,
    };

    let report_content = match fs::read_to_string(&report_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Find an active epic to attach the task to
    let epic_id = match crate::epics::list_epics(Some(crate::epics::EpicStatus::Active)) {
        Ok(epics) if !epics.is_empty() => epics[0].id.clone(),
        _ => return, // no active epic, skip
    };

    let task_id = format!(
        "{}-DESLOP-{}",
        epic_id,
        chrono::Utc::now().timestamp_millis() % 100000
    );

    let description = format!(
        "Desloppify scan found {} findings. Review and fix mechanical issues.\n\nReport: {}\n\n{}",
        scan.findings_count,
        report_path,
        &report_content[..report_content.len().min(2000)]
    );

    let _ = tasks::create_sqlite_task(tasks::CreateSqliteTaskInput {
        id: task_id,
        epic_id,
        title: format!("Fix {} desloppify findings", scan.findings_count),
        description: Some(description),
        priority: 9, // low priority — cleanup
        depends_on: Vec::new(),
        task_type: Some(tasks::SqliteTaskType::Refactor),
        archetype: Some("review".to_string()),
        footprint: tasks::TaskFootprint::default(),
    });

    let _ = workspace_root; // used for context
}

/// Write `.bacchus/ORCHESTRATOR.md` breadcrumb so the orchestrator protocol
/// survives context compaction (CLAUDE.md always stays loaded and points here).
fn write_orchestrator_breadcrumb(
    root: &std::path::Path,
    session: &Session,
    epic_id: Option<&str>,
    goal: Option<&str>,
) {
    let run_id = session.run_id.as_deref().unwrap_or("unknown");
    let max_concurrent = session.max_concurrent.unwrap_or(3);
    let started_at = &session.started_at;
    let epic = epic_id.unwrap_or("unset");
    let goal_text = goal.unwrap_or("No goal specified — check tasks.yaml for context.");

    let content = format!(
        r#"# Bacchus Orchestrator — Active Session

You are the orchestrator for this project. Follow this protocol.

## Session
- Run ID: {run_id}
- Started: {started_at}
- Max Concurrent: {max_concurrent}
- Epic: {epic}

## Goal
{goal_text}

## Hard Rules
- NEVER write, edit, or create source code files
- NEVER run `bacchus claim`, `bacchus next`, or `bacchus release`
- NEVER edit files in `.bacchus/workspaces/`
- Your ONLY output artifacts are `.bacchus/tasks.yaml` and messages to workers
- To get work done, spawn workers via `bacchus session spawn-workers`

## Monitor Loop
Run this cycle repeatedly until all tasks are closed:
```
bacchus status
bacchus list
bacchus process-releases
<run package manager install if releases were merged>
bacchus stale --minutes 15 --cleanup
bacchus events --limit 20
bacchus message list --agent orchestrator
bacchus task list --ready
bacchus session spawn-workers --count {max_concurrent}  # if ready tasks and slots available
```

## Task Planning Reminders
- High-impact tasks (features, refactors touching core logic) should include test-first instructions
- Same worker writes tests + implements — don't split into separate tasks for the same code region
- The pre-release quality gate runs project check/test/lint — workers cannot release if tests fail

## When All Tasks Closed
```
bacchus eval --days 7
bacchus session stop
bacchus epic set-status {epic} closed
```
"#
    );

    let path = root.join(".bacchus/ORCHESTRATOR.md");
    let _ = fs::write(&path, content);
}
