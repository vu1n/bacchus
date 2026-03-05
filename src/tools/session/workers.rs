//! Worker management: spawning, recovery, PID management, and reconciliation.

use crate::config::{find_workspace_root, sanitize_scope};
use crate::db::with_db;
use crate::tasks;
use crate::tools::orchestrator;
use crate::workers;
use crate::workspace;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::config::*;
use super::file::*;
use super::lifecycle::{start_session, stop_session};
use super::types::SessionMode;

#[derive(Debug, Default)]
pub(super) struct WorkerSpawnSummary {
    pub attempted: usize,
    pub launched: usize,
    pub skipped_backoff: usize,
    pub exhausted: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct WorkerRecoverySummary {
    pub scanned: usize,
    pub recovered: usize,
    pub reset_tasks: usize,
    pub pid_dead: usize,
    pub kill_attempted: usize,
    pub kill_succeeded: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct FailedWorkerReconcileSummary {
    pub candidates: usize,
    pub reopened: usize,
    pub errors: Vec<String>,
}

pub(super) struct RecoveryCycleResult {
    pub recovery: WorkerRecoverySummary,
    pub reconcile: FailedWorkerReconcileSummary,
    pub recovery_note: Option<String>,
    pub reconcile_note: Option<String>,
}

/// Run stale-worker recovery and failed-task reconciliation, returning raw data and formatted notes.
pub(super) fn run_recovery_cycle(
    workspace_root: &Path,
    run_id: &str,
    now_ms: i64,
    stale_cutoff_ms: i64,
    max_runtime_ms: Option<i64>,
    kill_stale: bool,
) -> RecoveryCycleResult {
    let recovery = recover_stale_workers(
        workspace_root,
        run_id,
        now_ms,
        stale_cutoff_ms,
        max_runtime_ms,
        kill_stale,
    );
    let reconcile = reconcile_failed_worker_tasks(workspace_root, run_id);

    let recovery_note =
        if recovery.recovered > 0 || !recovery.errors.is_empty() {
            Some(format!(
            "Recovered stale workers: {} (tasks reset: {}, dead pid: {}, kill: {}/{}, errors: {}).",
            recovery.recovered, recovery.reset_tasks, recovery.pid_dead,
            recovery.kill_succeeded, recovery.kill_attempted, recovery.errors.len()
        ))
        } else {
            None
        };
    let reconcile_note = if reconcile.reopened > 0 || !reconcile.errors.is_empty() {
        Some(format!(
            "Reopened tasks from failed workers: {} of {} candidates (errors: {}).",
            reconcile.reopened,
            reconcile.candidates,
            reconcile.errors.len()
        ))
    } else {
        None
    };

    RecoveryCycleResult {
        recovery,
        reconcile,
        recovery_note,
        reconcile_note,
    }
}

pub(super) fn generate_run_id(prefix: &str) -> String {
    format!(
        "{}-{}-{}",
        prefix,
        chrono::Utc::now().timestamp_millis(),
        std::process::id()
    )
}

pub(super) fn describe_existing_orchestrator_lease() -> Option<String> {
    let lease = tasks::get_orchestrator_lease().ok().flatten()?;
    let now = chrono::Utc::now().timestamp_millis();
    let remaining_ms = (lease.lease_expires_at - now).max(0);
    Some(format!(
        "holder={} expires_in={}s",
        lease.holder_id,
        remaining_ms / 1000
    ))
}

pub(super) fn count_active_claims(active_cutoff: i64) -> usize {
    with_db(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM tasks
             WHERE status = 'in_progress'
               AND claimed_by IS NOT NULL
               AND COALESCE(claimed_heartbeat_at, claimed_at, 0) >= ?1
               AND deleted_at IS NULL",
            [active_cutoff],
            |r| r.get::<_, i32>(0),
        )
    })
    .unwrap_or(0) as usize
}

pub(super) fn assign_agent_id(task_id: &str, attempt: i32) -> String {
    let token: String = task_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("agent-{}-{}", token, attempt.max(1))
}

pub(super) fn spawn_orchestrator_worker(
    workspace_root: &Path,
    run_id: &str,
    task: &tasks::SqliteTask,
    worker_cmd: &str,
    wcfg: &ResolvedWorkerConfig,
    now_ms: i64,
) -> Result<Option<String>, String> {
    let retry = workers::get_retry_state(run_id, &task.id)?;
    let next_attempt = retry.attempts + 1;

    let max_retries = wcfg.max_retries;
    if retry.attempts >= max_retries {
        let _ = tasks::reset_sqlite_task(&task.id, tasks::SqliteTaskStatus::Blocked);
        let _ = workers::create_worker_attempt(
            run_id,
            &task.id,
            "orchestrator",
            "orchestrator",
            worker_cmd,
            next_attempt,
        )
        .and_then(|worker_id| {
            workers::mark_worker_failed(
                worker_id,
                "worker retries exhausted; task moved to blocked",
                None,
            )
            .map(|_| ())
        });
        let _ = crate::events::record_event(
            Some(run_id),
            "orchestrator",
            "worker_spawn_exhausted",
            "task",
            &task.id,
            &serde_json::json!({
                "attempts": retry.attempts,
                "max_retries": max_retries
            }),
            Some(&format!("worker-spawn-exhausted:{}:{}", run_id, task.id)),
        );
        return Ok(Some("exhausted".to_string()));
    }

    if let Some(last_failed_at) = retry.last_failed_at {
        let backoff_ms = wcfg.retry_backoff_ms;
        if now_ms - last_failed_at < backoff_ms {
            return Ok(Some("backoff".to_string()));
        }
    }

    let agent_id = assign_agent_id(&task.id, next_attempt);
    tasks::claim_sqlite_task(&task.id, &agent_id).map_err(|e| e.to_string())?;

    let workspace = match workspace::create_workspace(workspace_root, &task.id) {
        Ok(ws) => ws,
        Err(e) => {
            let _ = tasks::reset_sqlite_task(&task.id, tasks::SqliteTaskStatus::Open);
            return Err(format!("Failed creating workspace for {}: {}", task.id, e));
        }
    };

    let scope_id = sanitize_scope(&format!("worker-{}-{}", run_id, task.id));
    let worker_id = workers::create_worker_attempt(
        run_id,
        &task.id,
        &agent_id,
        &scope_id,
        worker_cmd,
        next_attempt,
    )?;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("session")
        .arg("worker-run")
        .arg("--worker-id")
        .arg(worker_id.to_string())
        .arg("--run-id")
        .arg(run_id)
        .arg("--task-id")
        .arg(&task.id)
        .arg("--agent-id")
        .arg(&agent_id)
        .arg("--scope-id")
        .arg(&scope_id)
        .arg("--command")
        .arg(worker_cmd)
        .current_dir(workspace_root)
        .env("BACCHUS_SESSION_ID", &scope_id)
        .env(
            "BACCHUS_WORKSPACE_PATH",
            workspace.path.to_string_lossy().to_string(),
        )
        .stdin(Stdio::null());

    // Redirect stdout/stderr to per-task log file
    let logs_dir = workspace_root.join(".bacchus/logs");
    std::fs::create_dir_all(&logs_dir).ok();
    let log_path = logs_dir.join(format!("{}.log", task.id));
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(log_file) => match log_file.try_clone() {
            Ok(log_err) => {
                cmd.stdout(Stdio::from(log_file))
                    .stderr(Stdio::from(log_err));
            }
            Err(_) => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        },
        Err(_) => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }

    if let Ok(db_path) = std::env::var("BACCHUS_DB_PATH") {
        if !db_path.trim().is_empty() {
            cmd.env("BACCHUS_DB_PATH", db_path);
        }
    }

    match cmd.spawn() {
        Ok(child) => {
            let child_pid = child.id() as i64;
            let marked_running =
                workers::mark_worker_running(worker_id, Some(child_pid)).unwrap_or(false);
            if !marked_running {
                let _ = kill_pid_best_effort(child_pid);
                if let Ok(task_row) = tasks::get_sqlite_task(&task.id) {
                    if task_row.status == tasks::SqliteTaskStatus::InProgress
                        && task_row.claimed_by.as_deref() == Some(agent_id.as_str())
                    {
                        let _ = tasks::reset_sqlite_task(&task.id, tasks::SqliteTaskStatus::Open);
                        let _ = workspace::remove_workspace(workspace_root, &task.id);
                    }
                }
                let _ = crate::events::record_event(
                    Some(run_id),
                    "orchestrator",
                    "worker_spawn_fenced",
                    "task",
                    &task.id,
                    &serde_json::json!({
                        "worker_id": worker_id,
                        "agent_id": agent_id,
                        "scope_id": scope_id,
                        "attempt": next_attempt,
                        "pid": child_pid
                    }),
                    Some(&format!(
                        "worker-spawn-fenced:{}:{}:{}",
                        run_id, task.id, next_attempt
                    )),
                );
                return Err("Worker spawn fenced: worker row already finalized".to_string());
            }

            let _ = crate::events::record_event(
                Some(run_id),
                "orchestrator",
                "worker_spawned",
                "task",
                &task.id,
                &serde_json::json!({
                    "worker_id": worker_id,
                    "agent_id": agent_id,
                    "scope_id": scope_id,
                    "attempt": next_attempt,
                    "pid": child_pid
                }),
                Some(&format!(
                    "worker-spawned:{}:{}:{}",
                    run_id, task.id, next_attempt
                )),
            );
            Ok(None)
        }
        Err(e) => {
            let err_msg = format!("Failed to spawn worker process: {}", e);
            let _ = workers::mark_worker_failed(worker_id, &err_msg, None);
            let _ = tasks::reset_sqlite_task(&task.id, tasks::SqliteTaskStatus::Open);
            let _ = workspace::remove_workspace(workspace_root, &task.id);
            let _ = crate::events::record_event(
                Some(run_id),
                "orchestrator",
                "worker_spawn_failed",
                "task",
                &task.id,
                &serde_json::json!({
                    "worker_id": worker_id,
                    "attempt": next_attempt,
                    "error": err_msg
                }),
                Some(&format!(
                    "worker-spawn-failed:{}:{}:{}",
                    run_id, task.id, next_attempt
                )),
            );
            Err(err_msg)
        }
    }
}

pub(super) fn try_spawn_workers(
    workspace_root: &Path,
    run_id: &str,
    ready_tasks: &[tasks::SqliteTask],
    slots: usize,
    worker_cmd: &str,
    wcfg: &ResolvedWorkerConfig,
) -> WorkerSpawnSummary {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut summary = WorkerSpawnSummary::default();

    for task in ready_tasks.iter().take(slots) {
        summary.attempted += 1;
        match spawn_orchestrator_worker(workspace_root, run_id, task, worker_cmd, wcfg, now_ms) {
            Ok(Some(kind)) if kind == "backoff" => summary.skipped_backoff += 1,
            Ok(Some(kind)) if kind == "exhausted" => summary.exhausted += 1,
            Ok(Some(_)) => {}
            Ok(None) => summary.launched += 1,
            Err(e) => {
                summary.failed += 1;
                summary.errors.push(format!("{}: {}", task.id, e));
            }
        }
    }

    summary
}

pub(super) fn is_pid_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }

    #[cfg(target_os = "windows")]
    {
        let query = format!("tasklist /FI \"PID eq {}\"", pid);
        return Command::new("cmd")
            .args(["/C", &query])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|stdout| stdout.contains(&pid.to_string()))
            .unwrap_or(false);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let kill_zero = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if kill_zero {
            return true;
        }

        // Fallback: `kill -0` can fail with EPERM; check process table directly.
        Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|stdout| stdout.trim() == pid.to_string())
            .unwrap_or(false)
    }
}

pub(super) fn kill_pid_best_effort(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let term_ok = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !term_ok {
            return false;
        }
        std::thread::sleep(Duration::from_millis(150));
        if !is_pid_alive(pid) {
            return true;
        }
        Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

pub(super) fn recover_stale_workers(
    workspace_root: &Path,
    run_id: &str,
    now_ms: i64,
    stale_cutoff_ms: i64,
    max_runtime_ms: Option<i64>,
    kill_stale: bool,
) -> WorkerRecoverySummary {
    let mut summary = WorkerRecoverySummary::default();

    let snapshots = match workers::list_active_worker_snapshots(run_id) {
        Ok(v) => v,
        Err(e) => {
            summary.errors.push(format!("snapshot query failed: {}", e));
            return summary;
        }
    };
    summary.scanned = snapshots.len();

    for snapshot in snapshots {
        let task_status = snapshot.task_status.as_deref();
        let owner_matches = snapshot.task_claimed_by.as_deref() == Some(snapshot.agent_id.as_str());
        let task_last_seen = snapshot.task_last_seen_at.unwrap_or(0);
        let stale_state = task_status != Some("in_progress") || !owner_matches;
        let stale_heartbeat = task_last_seen < stale_cutoff_ms;
        let runtime_exceeded = max_runtime_ms
            .map(|max_ms| now_ms.saturating_sub(snapshot.started_at) > max_ms)
            .unwrap_or(false);
        let pid_dead = snapshot.pid.is_some_and(|pid| !is_pid_alive(pid));

        if !stale_state && !stale_heartbeat && !runtime_exceeded && !pid_dead {
            continue;
        }
        if pid_dead {
            summary.pid_dead += 1;
        }

        let reason = if stale_state {
            format!(
                "worker stale: task state mismatch (task_status={:?}, claimed_by={:?})",
                snapshot.task_status, snapshot.task_claimed_by
            )
        } else if pid_dead {
            format!("worker stale: pid {:?} is not alive", snapshot.pid)
        } else if runtime_exceeded {
            format!(
                "worker stale: runtime exceeded (started_at={}, now={}, max_runtime_ms={})",
                snapshot.started_at,
                now_ms,
                max_runtime_ms.unwrap_or_default()
            )
        } else {
            format!(
                "worker stale: claim heartbeat older than cutoff {}",
                stale_cutoff_ms
            )
        };

        let kill_attempted = kill_stale
            && !pid_dead
            && snapshot.pid.is_some()
            && (stale_heartbeat || runtime_exceeded)
            && task_status == Some("in_progress")
            && owner_matches;
        let mut kill_succeeded = false;
        if kill_attempted {
            summary.kill_attempted += 1;
            kill_succeeded = snapshot.pid.is_some_and(kill_pid_best_effort);
            if kill_succeeded {
                summary.kill_succeeded += 1;
            }
        }

        let transitioned = match workers::mark_worker_failed(snapshot.worker_id, &reason, None) {
            Ok(v) => v,
            Err(e) => {
                summary.errors.push(format!(
                    "worker {} (task {}): failed to mark worker failed: {}",
                    snapshot.worker_id, snapshot.task_id, e
                ));
                continue;
            }
        };
        if !transitioned {
            continue;
        }

        summary.recovered += 1;
        if task_status == Some("in_progress") && owner_matches {
            if let Err(e) =
                tasks::reset_sqlite_task(&snapshot.task_id, tasks::SqliteTaskStatus::Open)
            {
                summary.errors.push(format!(
                    "worker {} (task {}): failed to reset task: {}",
                    snapshot.worker_id, snapshot.task_id, e
                ));
            } else {
                summary.reset_tasks += 1;
                let _ = workspace::remove_workspace(workspace_root, &snapshot.task_id);
            }
        }

        let _ = crate::events::record_event(
            Some(run_id),
            "orchestrator",
            "worker_stale_recovered",
            "task",
            &snapshot.task_id,
            &serde_json::json!({
                "worker_id": snapshot.worker_id,
                "agent_id": snapshot.agent_id,
                "task_status": snapshot.task_status,
                "task_claimed_by": snapshot.task_claimed_by,
                "task_last_seen_at": snapshot.task_last_seen_at,
                "stale_cutoff_ms": stale_cutoff_ms,
                "stale_state": stale_state,
                "stale_heartbeat": stale_heartbeat,
                "runtime_exceeded": runtime_exceeded,
                "max_runtime_ms": max_runtime_ms,
                "pid_dead": pid_dead,
                "pid": snapshot.pid,
                "kill_stale_enabled": kill_stale,
                "kill_attempted": kill_attempted,
                "kill_succeeded": kill_succeeded,
                "reset_task": task_status == Some("in_progress") && owner_matches
            }),
            Some(&format!(
                "worker-stale-recovered:{}:{}",
                run_id, snapshot.worker_id
            )),
        );
    }

    summary
}

pub(super) fn reconcile_failed_worker_tasks(
    workspace_root: &Path,
    run_id: &str,
) -> FailedWorkerReconcileSummary {
    let mut summary = FailedWorkerReconcileSummary::default();
    let candidates = match workers::list_reopenable_failed_worker_tasks(run_id) {
        Ok(v) => v,
        Err(e) => {
            summary
                .errors
                .push(format!("failed-task query failed: {}", e));
            return summary;
        }
    };
    summary.candidates = candidates.len();

    for candidate in candidates {
        match tasks::reset_sqlite_task(&candidate.task_id, tasks::SqliteTaskStatus::Open) {
            Ok(_) => {
                summary.reopened += 1;
                let _ = workspace::remove_workspace(workspace_root, &candidate.task_id);
                let _ = crate::events::record_event(
                    Some(run_id),
                    "orchestrator",
                    "failed_worker_task_reopened",
                    "task",
                    &candidate.task_id,
                    &serde_json::json!({
                        "worker_id": candidate.worker_id,
                        "agent_id": candidate.agent_id
                    }),
                    Some(&format!(
                        "failed-worker-task-reopened:{}:{}",
                        run_id, candidate.worker_id
                    )),
                );
            }
            Err(e) => {
                summary.errors.push(format!(
                    "task {} (worker {}): reset failed: {}",
                    candidate.task_id, candidate.worker_id, e
                ));
            }
        }
    }

    summary
}

/// Spawn ready workers once for the active orchestrator session.
pub fn spawn_workers_once(
    count: Option<usize>,
    dry_run: bool,
) -> Result<serde_json::Value, String> {
    let session = match session_path().filter(|p| p.exists()) {
        Some(path) => read_session(&path)?,
        None => return Err("No active session".to_string()),
    };

    if session.mode != SessionMode::Orchestrator {
        return Err(format!(
            "Current session mode is '{}'; spawn-workers requires an orchestrator session",
            session.mode
        ));
    }

    let run_id = session
        .run_id
        .clone()
        .unwrap_or_else(|| session.started_at.clone());
    let max_concurrent = session.max_concurrent.unwrap_or(3).max(1) as usize;
    let requested = count.unwrap_or(max_concurrent).max(1);
    let now = chrono::Utc::now().timestamp_millis();
    let active_cutoff = now - tasks::CLAIM_HEARTBEAT_TIMEOUT_MS;

    let lease_ok =
        tasks::try_acquire_orchestrator_lease(&run_id, configured_orchestrator_lease_ttl_ms())
            .map_err(|e| e.to_string())?;
    if !lease_ok {
        let details =
            describe_existing_orchestrator_lease().unwrap_or_else(|| "holder=unknown".to_string());
        return Err(format!(
            "Another orchestrator leader lease is active ({}).",
            details
        ));
    }

    let workspace_root = find_workspace_root().ok_or("No workspace root found")?;
    let wcfg = resolve_worker_config(Some(&workspace_root));
    let worker_stale_cutoff = active_cutoff - wcfg.stale_grace_ms;
    let cycle = run_recovery_cycle(
        &workspace_root,
        &run_id,
        now,
        worker_stale_cutoff,
        wcfg.max_runtime_ms,
        wcfg.kill_stale,
    );
    let recovery_note = cycle.recovery_note;
    let failed_reconcile_note = cycle.reconcile_note;
    let recovery = cycle.recovery;
    let failed_task_reconcile = cycle.reconcile;
    let release_note =
        match orchestrator::process_ready_releases(&workspace_root, Some(20), Some(&run_id)) {
            Ok(summary)
                if summary.reconciled > 0
                    || summary.merged > 0
                    || summary.conflicts > 0
                    || summary.failed > 0 =>
            {
                Some(format!(
                    "Processed releases: reconciled {}, merged {}, conflicts {}, failed {}.",
                    summary.reconciled, summary.merged, summary.conflicts, summary.failed
                ))
            }
            Ok(_) => None,
            Err(e) => Some(format!("Release processing error: {}.", e)),
        };
    let active_claims = count_active_claims(active_cutoff);
    let active_workers = workers::count_active_workers(&run_id).unwrap_or(0);
    let ready_tasks = tasks::get_ready_sqlite_tasks(None).unwrap_or_default();
    let candidate_slots = max_concurrent.saturating_sub(active_claims);
    let spawn_slots = requested.min(candidate_slots);
    let candidate_tasks: Vec<String> = ready_tasks
        .iter()
        .take(spawn_slots.min(ready_tasks.len()))
        .map(|t| t.id.clone())
        .collect();

    if dry_run {
        return Ok(serde_json::json!({
            "success": true,
            "dry_run": true,
            "run_id": run_id,
            "requested": requested,
            "max_concurrent": max_concurrent,
            "active_claims": active_claims,
            "active_workers": active_workers,
            "available_slots": candidate_slots,
            "ready_count": ready_tasks.len(),
            "candidate_tasks": candidate_tasks,
            "release_note": release_note,
            "recovery_note": recovery_note,
            "failed_reconcile_note": failed_reconcile_note,
            "stale_worker_recovery": {
                "scanned": recovery.scanned,
                "recovered": recovery.recovered,
                "reset_tasks": recovery.reset_tasks,
                "pid_dead": recovery.pid_dead,
                "kill_attempted": recovery.kill_attempted,
                "kill_succeeded": recovery.kill_succeeded,
                "errors": recovery.errors
            },
            "failed_worker_reconcile": {
                "candidates": failed_task_reconcile.candidates,
                "reopened": failed_task_reconcile.reopened,
                "errors": failed_task_reconcile.errors
            }
        }));
    }

    let worker_cmd = wcfg.cmd.as_deref().ok_or(
        "worker.cmd is not configured. Set worker.cmd in .bacchus/config.yaml (or BACCHUS_WORKER_CMD env var) and rerun `bacchus session spawn-workers`.",
    )?;

    let summary = if spawn_slots == 0 {
        WorkerSpawnSummary::default()
    } else {
        try_spawn_workers(
            &workspace_root,
            &run_id,
            &ready_tasks,
            spawn_slots,
            worker_cmd,
            &wcfg,
        )
    };

    let post_active_claims = count_active_claims(active_cutoff);
    let post_active_workers = workers::count_active_workers(&run_id).unwrap_or(0);
    let post_ready = tasks::get_ready_sqlite_tasks(None)
        .unwrap_or_default()
        .len();

    Ok(serde_json::json!({
        "success": true,
        "dry_run": false,
        "run_id": run_id,
        "requested": requested,
        "max_concurrent": max_concurrent,
        "active_claims_before": active_claims,
        "active_workers_before": active_workers,
        "available_slots_before": candidate_slots,
        "ready_count_before": ready_tasks.len(),
        "candidate_tasks": candidate_tasks,
        "spawn_summary": {
            "attempted": summary.attempted,
            "launched": summary.launched,
            "failed": summary.failed,
            "backoff": summary.skipped_backoff,
            "exhausted": summary.exhausted,
            "errors": summary.errors
        },
        "active_claims_after": post_active_claims,
        "active_workers_after": post_active_workers,
        "ready_count_after": post_ready,
        "release_note": release_note,
        "recovery_note": recovery_note,
        "failed_reconcile_note": failed_reconcile_note,
        "stale_worker_recovery": {
            "scanned": recovery.scanned,
            "recovered": recovery.recovered,
            "reset_tasks": recovery.reset_tasks,
            "pid_dead": recovery.pid_dead,
            "kill_attempted": recovery.kill_attempted,
            "kill_succeeded": recovery.kill_succeeded,
            "errors": recovery.errors
        },
        "failed_worker_reconcile": {
            "candidates": failed_task_reconcile.candidates,
            "reopened": failed_task_reconcile.reopened,
            "errors": failed_task_reconcile.errors
        }
    }))
}

/// Internal worker wrapper entrypoint.
///
/// Runs a configured worker command for a claimed task and updates worker lifecycle state.
pub fn run_worker_command(
    worker_id: i64,
    run_id: &str,
    task_id: &str,
    agent_id: &str,
    scope_id: &str,
    command: &str,
) -> Result<String, String> {
    let _ = scope_id;
    let workspace_root = find_workspace_root().ok_or("No workspace root found")?;
    let workspace_path = workspace::get_workspaces_dir(&workspace_root).join(task_id);

    let status = workers::get_worker_status(worker_id)?;
    if !matches!(status.as_deref(), Some("launching" | "running")) {
        return Ok(format!(
            "Worker {} is no longer active (status={:?}); command execution skipped",
            worker_id, status
        ));
    }

    // Ensure worker-scoped agent session for heartbeat and stop-hook semantics.
    let _ = start_session(
        SessionMode::Agent,
        Some(task_id),
        1,
        Some(agent_id),
        None,
        None,
    );

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };

    cmd.current_dir(if workspace_path.exists() {
        workspace_path.as_path()
    } else {
        workspace_root.as_path()
    })
    .env("BACCHUS_TASK_ID", task_id)
    .env("BACCHUS_AGENT_ID", agent_id)
    .env("BACCHUS_RUN_ID", run_id)
    .env("BACCHUS_WORKER_ID", worker_id.to_string())
    .env(
        "BACCHUS_WORKSPACE_ROOT",
        workspace_root.to_string_lossy().to_string(),
    )
    .env(
        "BACCHUS_WORKSPACE_PATH",
        workspace_path.to_string_lossy().to_string(),
    );

    let output = cmd.output().map_err(|e| e.to_string())?;
    let exit_code = output.status.code();

    // Append captured stdout/stderr to per-task log file
    {
        let logs_dir = workspace_root.join(".bacchus/logs");
        std::fs::create_dir_all(&logs_dir).ok();
        let log_path = logs_dir.join(format!("{}.log", task_id));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            use std::io::Write;
            if !output.stdout.is_empty() {
                let _ = f.write_all(&output.stdout);
            }
            if !output.stderr.is_empty() {
                let _ = writeln!(f, "\n--- stderr ---");
                let _ = f.write_all(&output.stderr);
            }
        }
    }

    if output.status.success() {
        let transitioned = workers::mark_worker_completed(worker_id, exit_code).unwrap_or(false);
        if transitioned {
            let _ = crate::events::record_event(
                Some(run_id),
                "worker",
                "worker_exited",
                "task",
                task_id,
                &serde_json::json!({
                    "worker_id": worker_id,
                    "agent_id": agent_id,
                    "success": true,
                    "exit_code": exit_code,
                    "fenced": false
                }),
                Some(&format!("worker-exited:{}:{}:ok", run_id, worker_id)),
            );
            let _ = stop_session();
            return Ok(format!("Worker {} completed", worker_id));
        }
        let _ = crate::events::record_event(
            Some(run_id),
            "worker",
            "worker_exit_ignored",
            "task",
            task_id,
            &serde_json::json!({
                "worker_id": worker_id,
                "agent_id": agent_id,
                "success": true,
                "exit_code": exit_code,
                "reason": "worker row already finalized"
            }),
            Some(&format!("worker-exit-ignored:{}:{}:ok", run_id, worker_id)),
        );
        let _ = stop_session();
        return Ok(format!(
            "Worker {} completed but state was already finalized; exit ignored",
            worker_id
        ));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut failure = format!("Worker command failed with exit code {:?}", exit_code);
    if !stderr.is_empty() {
        failure = format!("{}; stderr: {}", failure, stderr);
    } else if !stdout.is_empty() {
        failure = format!("{}; stdout: {}", failure, stdout);
    }

    let transitioned = workers::mark_worker_failed(worker_id, &failure, exit_code).unwrap_or(false);

    if transitioned {
        if let Ok(task) = tasks::get_sqlite_task(task_id) {
            if task.status == tasks::SqliteTaskStatus::InProgress
                && task.claimed_by.as_deref() == Some(agent_id)
            {
                let _ = tasks::reset_sqlite_task(task_id, tasks::SqliteTaskStatus::Open);
                let _ = workspace::remove_workspace(&workspace_root, task_id);
            }
        }
    }

    let _ = crate::events::record_event(
        Some(run_id),
        "worker",
        "worker_exited",
        "task",
        task_id,
        &serde_json::json!({
            "worker_id": worker_id,
            "agent_id": agent_id,
            "success": false,
            "exit_code": exit_code,
            "error": failure,
            "fenced": !transitioned
        }),
        Some(&format!("worker-exited:{}:{}:err", run_id, worker_id)),
    );

    let _ = stop_session();
    if transitioned {
        Err(format!("Worker {} failed: {}", worker_id, failure))
    } else {
        Ok(format!(
            "Worker {} failed but state was already finalized; exit ignored",
            worker_id
        ))
    }
}
