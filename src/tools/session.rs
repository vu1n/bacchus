//! Session management for stop hooks
//!
//! Manages scoped session files under .bacchus/sessions/ for persistent session state.

use crate::db::with_db;
use crate::handles;
use crate::messages;
use crate::tasks;
use crate::tools::orchestrator;
use crate::workers;
use crate::workspace;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const DEFAULT_AGENT_HEARTBEAT_INTERVAL_MS: u64 = 30_000;
const DEFAULT_ORCHESTRATOR_LEASE_RENEW_INTERVAL_MS: u64 = 30_000;
const DEFAULT_WORKER_RETRY_BACKOFF_MS: i64 = 60_000;
const DEFAULT_WORKER_MAX_RETRIES: i32 = 3;
const DEFAULT_WORKER_STALE_GRACE_MS: i64 = 60_000;
const DEFAULT_WORKER_KILL_STALE: bool = false;
const SESSION_SCOPE_ENV_KEYS: [&str; 3] = [
    "BACCHUS_SESSION_ID",
    "CLAUDE_SESSION_ID",
    "CLAUDE_CONVERSATION_ID",
];

/// Session state stored in scoped .bacchus/sessions/<scope>.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>, // For architect mode (persistent identity)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_heartbeat_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_lease_token: Option<String>,
    pub started_at: String,
}

/// Output for hook check command
#[derive(Debug, Serialize, Deserialize)]
pub struct HookCheckOutput {
    pub decision: String, // "approve" or "block"
    pub reason: String,
}

/// Find workspace root for session management
///
/// Priority:
/// 1. CLAUDE_PROJECT_DIR env var (set by Claude Code for plugins/hooks)
/// 2. Walk up from CWD looking for .bacchus or .git
fn find_workspace_root() -> Option<std::path::PathBuf> {
    // First check CLAUDE_PROJECT_DIR (set by Claude Code for hooks/plugins)
    if let Ok(project_dir) = std::env::var("CLAUDE_PROJECT_DIR") {
        let path = std::path::PathBuf::from(&project_dir);
        if path.exists() {
            return Some(path);
        }
    }

    // Walk up from current directory
    let mut current = std::env::current_dir().ok()?;
    loop {
        // Check for bacchus marker first
        if current.join(".bacchus").exists() {
            return Some(current);
        }
        // Fall back to .git as project root indicator
        let git_path = current.join(".git");
        if git_path.exists() && git_path.is_dir() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn sanitize_scope(scope: &str) -> String {
    let mut out = String::with_capacity(scope.len());
    for ch in scope.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

pub fn current_session_scope_id() -> String {
    for key in SESSION_SCOPE_ENV_KEYS {
        if let Ok(v) = std::env::var(key) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return sanitize_scope(trimmed);
            }
        }
    }
    "default".to_string()
}

fn session_file_path_for_scope(scope: &str) -> Option<std::path::PathBuf> {
    find_workspace_root().map(|root| {
        root.join(".bacchus")
            .join("sessions")
            .join(format!("{}.json", sanitize_scope(scope)))
    })
}

fn scoped_session_path() -> Option<std::path::PathBuf> {
    session_file_path_for_scope(&current_session_scope_id())
}

fn legacy_session_path() -> Option<std::path::PathBuf> {
    find_workspace_root().map(|root| root.join(".bacchus/session.json"))
}

fn session_path() -> Option<std::path::PathBuf> {
    if let Some(path) = scoped_session_path() {
        if path.exists() {
            return Some(path);
        }
    }
    legacy_session_path().filter(|p| p.exists())
}

fn configured_agent_heartbeat_interval_ms() -> u64 {
    std::env::var("BACCHUS_AGENT_HEARTBEAT_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_AGENT_HEARTBEAT_INTERVAL_MS)
}

fn configured_orchestrator_lease_ttl_ms() -> i64 {
    std::env::var("BACCHUS_ORCHESTRATOR_LEASE_TTL_MS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(tasks::ORCHESTRATOR_LEASE_TTL_MS)
}

fn configured_orchestrator_lease_interval_ms() -> u64 {
    std::env::var("BACCHUS_ORCHESTRATOR_LEASE_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_ORCHESTRATOR_LEASE_RENEW_INTERVAL_MS)
}

fn configured_orchestrator_auto_spawn() -> bool {
    std::env::var("BACCHUS_ORCHESTRATOR_AUTO_SPAWN")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        })
        .unwrap_or(true)
}

fn configured_worker_command() -> Option<String> {
    std::env::var("BACCHUS_WORKER_CMD")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn configured_worker_retry_backoff_ms() -> i64 {
    std::env::var("BACCHUS_WORKER_RETRY_BACKOFF_MS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_WORKER_RETRY_BACKOFF_MS)
}

fn configured_worker_max_retries() -> i32 {
    std::env::var("BACCHUS_WORKER_MAX_RETRIES")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_WORKER_MAX_RETRIES)
}

fn configured_worker_stale_grace_ms() -> i64 {
    std::env::var("BACCHUS_WORKER_STALE_GRACE_MS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(DEFAULT_WORKER_STALE_GRACE_MS)
}

fn configured_worker_max_runtime_ms() -> Option<i64> {
    std::env::var("BACCHUS_WORKER_MAX_RUNTIME_MS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
}

fn configured_worker_kill_stale() -> bool {
    std::env::var("BACCHUS_WORKER_KILL_STALE")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        })
        .unwrap_or(DEFAULT_WORKER_KILL_STALE)
}

#[derive(Debug, Default)]
struct WorkerSpawnSummary {
    attempted: usize,
    launched: usize,
    skipped_backoff: usize,
    exhausted: usize,
    failed: usize,
    errors: Vec<String>,
}

#[derive(Debug, Default)]
struct WorkerRecoverySummary {
    scanned: usize,
    recovered: usize,
    reset_tasks: usize,
    pid_dead: usize,
    kill_attempted: usize,
    kill_succeeded: usize,
    errors: Vec<String>,
}

#[derive(Debug, Default)]
struct FailedWorkerReconcileSummary {
    candidates: usize,
    reopened: usize,
    errors: Vec<String>,
}

fn generate_run_id(prefix: &str) -> String {
    format!(
        "{}-{}-{}",
        prefix,
        chrono::Utc::now().timestamp_millis(),
        std::process::id()
    )
}

fn read_session(path: &Path) -> Result<Session, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content).map_err(|e| e.to_string())
}

fn write_session(path: &Path, session: &Session) -> Result<(), String> {
    let json = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

fn session_age_minutes(session: &Session, path: &Path) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&session.started_at) {
        let started_ms = dt.timestamp_millis();
        let now_ms = chrono::Utc::now().timestamp_millis();
        return Some(((now_ms - started_ms).max(0)) / 60000);
    }

    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let now = std::time::SystemTime::now();
    let age_secs = now.duration_since(modified).ok()?.as_secs() as i64;
    Some(age_secs / 60)
}

fn sessions_dir() -> Option<std::path::PathBuf> {
    find_workspace_root().map(|root| root.join(".bacchus").join("sessions"))
}

fn same_default_scope_identity(existing: &Session, new_session: &Session) -> bool {
    if existing.mode != new_session.mode {
        return false;
    }
    if existing.task_id != new_session.task_id {
        return false;
    }
    match (&existing.agent_id, &new_session.agent_id) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

fn describe_existing_orchestrator_lease() -> Option<String> {
    let lease = tasks::get_orchestrator_lease().ok().flatten()?;
    let now = chrono::Utc::now().timestamp_millis();
    let remaining_ms = (lease.lease_expires_at - now).max(0);
    Some(format!(
        "holder={} expires_in={}s",
        lease.holder_id,
        remaining_ms / 1000
    ))
}

fn count_active_claims(active_cutoff: i64) -> usize {
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

fn assign_agent_id(task_id: &str, attempt: i32) -> String {
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

fn spawn_orchestrator_worker(
    workspace_root: &Path,
    run_id: &str,
    task: &tasks::SqliteTask,
    worker_cmd: &str,
    now_ms: i64,
) -> Result<Option<String>, String> {
    let retry = workers::get_retry_state(run_id, &task.id)?;
    let next_attempt = retry.attempts + 1;

    let max_retries = configured_worker_max_retries();
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
        let backoff_ms = configured_worker_retry_backoff_ms();
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
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

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
                        let _ = workspace::remove_workspace(workspace_root, &task.id, true);
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
            let _ = workspace::remove_workspace(workspace_root, &task.id, true);
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

fn try_spawn_workers(
    workspace_root: &Path,
    run_id: &str,
    ready_tasks: &[tasks::SqliteTask],
    slots: usize,
    worker_cmd: &str,
) -> WorkerSpawnSummary {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut summary = WorkerSpawnSummary::default();

    for task in ready_tasks.iter().take(slots) {
        summary.attempted += 1;
        match spawn_orchestrator_worker(workspace_root, run_id, task, worker_cmd, now_ms) {
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

fn is_pid_alive(pid: i64) -> bool {
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

fn kill_pid_best_effort(pid: i64) -> bool {
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

fn recover_stale_workers(
    workspace_root: &Path,
    run_id: &str,
    now_ms: i64,
    stale_cutoff_ms: i64,
    max_runtime_ms: Option<i64>,
) -> WorkerRecoverySummary {
    let mut summary = WorkerRecoverySummary::default();
    let kill_stale = configured_worker_kill_stale();

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
                let _ = workspace::remove_workspace(workspace_root, &snapshot.task_id, true);
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

fn reconcile_failed_worker_tasks(
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
                let _ = workspace::remove_workspace(workspace_root, &candidate.task_id, true);
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
                    if session.mode == "orchestrator" {
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
                    if session.mode == "orchestrator" {
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

fn spawn_agent_heartbeat_loop(
    task_id: &str,
    agent_id: &str,
    token: &str,
    interval_ms: u64,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .arg("session")
        .arg("heartbeat-loop")
        .arg("--task-id")
        .arg(task_id)
        .arg("--agent-id")
        .arg(agent_id)
        .arg("--token")
        .arg(token)
        .arg("--interval-ms")
        .arg(interval_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn spawn_orchestrator_lease_loop(
    run_id: &str,
    token: &str,
    interval_ms: u64,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .arg("session")
        .arg("lease-loop")
        .arg("--run-id")
        .arg(run_id)
        .arg("--token")
        .arg(token)
        .arg("--interval-ms")
        .arg(interval_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Attach (or refresh) a background heartbeat loop for the active agent session.
///
/// This is used by `session start agent` and by `claim` when session start happened first.
pub fn attach_agent_session_heartbeat(task_id: &str, agent_id: &str) -> Result<(), String> {
    let path = match session_path() {
        Some(p) if p.exists() => p,
        _ => return Ok(()),
    };

    let mut session = read_session(&path)?;
    if session.mode != "agent" || session.task_id.as_deref() != Some(task_id) {
        return Ok(());
    }

    let token = generate_run_id("agent-hb");
    session.agent_id = Some(agent_id.to_string());
    session.agent_heartbeat_token = Some(token.clone());
    write_session(&path, &session)?;

    spawn_agent_heartbeat_loop(
        task_id,
        agent_id,
        &token,
        configured_agent_heartbeat_interval_ms(),
    )
}

/// Internal long-running heartbeat worker.
pub fn run_agent_heartbeat_loop(
    task_id: &str,
    agent_id: &str,
    token: &str,
    interval_ms: u64,
) -> Result<String, String> {
    let interval = Duration::from_millis(interval_ms.max(100));

    loop {
        let path = match session_path() {
            Some(p) if p.exists() => p,
            _ => break,
        };

        let session = match read_session(&path) {
            Ok(s) => s,
            Err(_) => break,
        };

        // Exit if this loop is no longer the session's active heartbeat owner.
        if session.mode != "agent"
            || session.task_id.as_deref() != Some(task_id)
            || session.agent_id.as_deref() != Some(agent_id)
            || session.agent_heartbeat_token.as_deref() != Some(token)
        {
            break;
        }

        if tasks::heartbeat_sqlite_task(task_id, agent_id).is_err() {
            break;
        }

        thread::sleep(interval);
    }

    Ok("Agent heartbeat loop exited".to_string())
}

/// Internal long-running orchestrator leader lease renewer.
pub fn run_orchestrator_lease_loop(
    run_id: &str,
    token: &str,
    interval_ms: u64,
) -> Result<String, String> {
    let interval = Duration::from_millis(interval_ms.max(100));
    let ttl_ms = configured_orchestrator_lease_ttl_ms();

    loop {
        let path = match session_path() {
            Some(p) if p.exists() => p,
            _ => break,
        };

        let session = match read_session(&path) {
            Ok(s) => s,
            Err(_) => break,
        };

        if session.mode != "orchestrator"
            || session.run_id.as_deref() != Some(run_id)
            || session.orchestrator_lease_token.as_deref() != Some(token)
        {
            break;
        }

        match tasks::try_acquire_orchestrator_lease(run_id, ttl_ms) {
            Ok(true) => {}
            Ok(false) | Err(_) => break,
        }

        thread::sleep(interval);
    }

    Ok("Orchestrator lease loop exited".to_string())
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
    let _ = start_session("agent", Some(task_id), 1, Some(agent_id));

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
                let _ = workspace::remove_workspace(&workspace_root, task_id, true);
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

/// Start a session
pub fn start_session(
    mode: &str,
    task_id: Option<&str>,
    max_concurrent: i32,
    agent_id: Option<&str>,
) -> Result<String, String> {
    let root = find_workspace_root().ok_or("No workspace root found")?;
    let bacchus_dir = root.join(".bacchus");
    fs::create_dir_all(&bacchus_dir).map_err(|e| e.to_string())?;
    let session_file = scoped_session_path().ok_or("No workspace root found")?;
    if let Some(parent) = session_file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let session = match mode {
        "agent" => {
            let task_id = task_id.ok_or("task_id required for agent mode")?;
            let claim_owner = tasks::get_sqlite_task(task_id)
                .ok()
                .and_then(|task| task.claimed_by);
            Session {
                mode: "agent".to_string(),
                task_id: Some(task_id.to_string()),
                max_concurrent: None,
                agent_id: agent_id.map(String::from).or(claim_owner),
                run_id: Some(generate_run_id("agent")),
                agent_heartbeat_token: None,
                orchestrator_lease_token: None,
                started_at: chrono::Utc::now().to_rfc3339(),
            }
        }
        "orchestrator" => {
            let run_id = generate_run_id("orchestrator");
            let acquired = tasks::try_acquire_orchestrator_lease(
                &run_id,
                configured_orchestrator_lease_ttl_ms(),
            )
            .map_err(|e| e.to_string())?;
            if !acquired {
                let details = describe_existing_orchestrator_lease()
                    .unwrap_or_else(|| "holder=unknown".to_string());
                return Err(format!(
                    "Another orchestrator leader lease is active ({}).",
                    details
                ));
            }

            Session {
                mode: "orchestrator".to_string(),
                task_id: None,
                max_concurrent: Some(max_concurrent),
                agent_id: None,
                run_id: Some(run_id),
                agent_heartbeat_token: None,
                orchestrator_lease_token: Some(generate_run_id("lease")),
                started_at: chrono::Utc::now().to_rfc3339(),
            }
        }
        "architect" => {
            let agent_id = agent_id.ok_or("agent_id required for architect mode")?;
            Session {
                mode: "architect".to_string(),
                task_id: None,
                max_concurrent: None,
                agent_id: Some(agent_id.to_string()),
                run_id: Some(generate_run_id("architect")),
                agent_heartbeat_token: None,
                orchestrator_lease_token: None,
                started_at: chrono::Utc::now().to_rfc3339(),
            }
        }
        _ => {
            return Err(format!(
                "Unknown mode: {}. Use 'agent', 'orchestrator', or 'architect'",
                mode
            ))
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
        if session.mode == "orchestrator" {
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
    if mode == "agent" {
        if let (Some(task), Some(owner)) = (session.task_id.as_deref(), session.agent_id.as_deref())
        {
            if let Err(e) = attach_agent_session_heartbeat(task, owner) {
                message = format!("{} (heartbeat loop unavailable: {})", message, e);
            }
        }
    } else if mode == "orchestrator" {
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
    }

    Ok(message)
}

/// Stop the session and clean up session-scoped handles
pub fn stop_session() -> Result<String, String> {
    if let Some(path) = session_path().filter(|p| p.exists()) {
        let session = read_session(&path).ok();

        if let Some(s) = session.as_ref() {
            if s.mode == "orchestrator" {
                if let Some(run_id) = s.run_id.as_deref() {
                    let _ = tasks::release_orchestrator_lease(run_id);
                    let _ = workers::fail_active_workers(run_id, "orchestrator session stopped");
                    if let Some(root) = find_workspace_root() {
                        let _ = reconcile_failed_worker_tasks(&root, run_id);
                    }
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

/// Check if session should block exit (for stop hook)
pub fn check_session() -> HookCheckOutput {
    // Read session file
    let session = match session_path() {
        Some(path) if path.exists() => match read_session(&path) {
            Ok(s) => s,
            Err(_) => {
                return HookCheckOutput {
                    decision: "approve".to_string(),
                    reason: "Invalid session file".to_string(),
                }
            }
        },
        _ => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "No bacchus session active".to_string(),
            }
        }
    };

    match session.mode.as_str() {
        "agent" => check_agent_session(&session),
        "orchestrator" => check_orchestrator_session(&session),
        "architect" => check_architect_session(&session),
        _ => HookCheckOutput {
            decision: "approve".to_string(),
            reason: format!("Unknown session mode: {}", session.mode),
        },
    }
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

    if session.mode != "orchestrator" {
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
    let worker_stale_cutoff = active_cutoff - configured_worker_stale_grace_ms();

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
    let recovery = recover_stale_workers(
        &workspace_root,
        &run_id,
        now,
        worker_stale_cutoff,
        configured_worker_max_runtime_ms(),
    );
    let failed_task_reconcile = reconcile_failed_worker_tasks(&workspace_root, &run_id);
    let recovery_note = if recovery.recovered > 0 || !recovery.errors.is_empty() {
        Some(format!(
            "Recovered stale workers: {} (tasks reset: {}, dead pid: {}, kill: {}/{}, errors: {}).",
            recovery.recovered,
            recovery.reset_tasks,
            recovery.pid_dead,
            recovery.kill_succeeded,
            recovery.kill_attempted,
            recovery.errors.len()
        ))
    } else {
        None
    };
    let failed_reconcile_note =
        if failed_task_reconcile.reopened > 0 || !failed_task_reconcile.errors.is_empty() {
            Some(format!(
                "Reopened tasks from failed workers: {} of {} candidates (errors: {}).",
                failed_task_reconcile.reopened,
                failed_task_reconcile.candidates,
                failed_task_reconcile.errors.len()
            ))
        } else {
            None
        };
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

    let worker_cmd = configured_worker_command().ok_or(
        "BACCHUS_WORKER_CMD is not set. Set it and rerun `bacchus session spawn-workers`.",
    )?;

    let summary = if spawn_slots == 0 {
        WorkerSpawnSummary::default()
    } else {
        try_spawn_workers(
            &workspace_root,
            &run_id,
            &ready_tasks,
            spawn_slots,
            &worker_cmd,
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

fn check_agent_session(session: &Session) -> HookCheckOutput {
    let task_id = match &session.task_id {
        Some(id) => id,
        None => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "No task ID in session".to_string(),
            }
        }
    };

    // Get workspace root for task lookup
    let _workspace_root = match find_workspace_root() {
        Some(root) => root,
        None => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "Cannot find workspace root".to_string(),
            }
        }
    };

    // Check task status
    match tasks::get_sqlite_task(task_id) {
        Ok(task) => {
            if let Some(owner) = task.claimed_by.as_deref() {
                if let Some(session_agent_id) = session.agent_id.as_deref() {
                    if owner != session_agent_id {
                        let _ = stop_session();
                        return HookCheckOutput {
                            decision: "approve".to_string(),
                            reason: format!(
                                "Task {} is now owned by {} (session owner was {}). Session cleared.",
                                task_id, owner, session_agent_id
                            ),
                        };
                    }
                } else {
                    let _ = attach_agent_session_heartbeat(task_id, owner);
                }
            }

            match task.status {
                tasks::SqliteTaskStatus::Closed => {
                    let _ = stop_session();
                    HookCheckOutput {
                        decision: "approve".to_string(),
                        reason: format!("Task {} is closed. Session cleared.", task_id),
                    }
                }
                tasks::SqliteTaskStatus::Blocked => {
                    let _ = stop_session();
                    HookCheckOutput {
                        decision: "approve".to_string(),
                        reason: format!("Task {} is blocked. Session cleared.", task_id),
                    }
                }
                tasks::SqliteTaskStatus::InProgress => {
                    if let Some(session_agent_id) = session.agent_id.as_deref() {
                        if task.claimed_by.as_deref() == Some(session_agent_id) {
                            let _ = tasks::heartbeat_sqlite_task(task_id, session_agent_id);
                        }
                    }
                    HookCheckOutput {
                        decision: "block".to_string(),
                        reason: format!(
                            "Task {} is in_progress. Continue working, then run 'bacchus release {} --status done' or '--status blocked'.",
                            task_id, task_id
                        ),
                    }
                }
                tasks::SqliteTaskStatus::ReadyForRelease => HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} is ready_for_release. Wait for orchestrator merge, or run 'bacchus process-releases' from an orchestrator flow.",
                        task_id
                    ),
                },
                tasks::SqliteTaskStatus::Releasing => HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} is currently releasing. Wait for merge/conflict outcome before exiting.",
                        task_id
                    ),
                },
                tasks::SqliteTaskStatus::NeedsResolution => HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} needs conflict resolution. Resolve conflicts, then run 'bacchus resolve {}' (or 'bacchus abort {}' to continue editing).",
                        task_id, task_id, task_id
                    ),
                },
                tasks::SqliteTaskStatus::Open | tasks::SqliteTaskStatus::Draft => HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} is '{}'. Reclaim it with 'bacchus claim {} <agent_id>' or stop the session if this task is no longer assigned.",
                        task_id,
                        task.status.as_str(),
                        task_id
                    ),
                },
            }
        }
        Err(e) => HookCheckOutput {
            decision: "approve".to_string(),
            reason: format!("Cannot check task status: {}", e),
        },
    }
}

fn check_orchestrator_session(session: &Session) -> HookCheckOutput {
    let max_concurrent = session.max_concurrent.unwrap_or(3);
    let now = chrono::Utc::now().timestamp_millis();
    let active_cutoff = now - tasks::CLAIM_HEARTBEAT_TIMEOUT_MS;
    let worker_stale_cutoff = active_cutoff - configured_worker_stale_grace_ms();
    let run_id = session
        .run_id
        .as_deref()
        .unwrap_or(session.started_at.as_str());

    match tasks::try_acquire_orchestrator_lease(run_id, configured_orchestrator_lease_ttl_ms()) {
        Ok(true) => {}
        Ok(false) => {
            let details = describe_existing_orchestrator_lease()
                .unwrap_or_else(|| "holder=unknown".to_string());
            let _ = stop_session();
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: format!(
                    "Another orchestrator leader lease is active ({}). Session cleared.",
                    details
                ),
            };
        }
        Err(e) => {
            return HookCheckOutput {
                decision: "block".to_string(),
                reason: format!("Failed to renew orchestrator lease: {}", e),
            };
        }
    }

    // Get workspace root for task lookup
    let workspace_root = match find_workspace_root() {
        Some(root) => root,
        None => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "Cannot find workspace root".to_string(),
            }
        }
    };

    let recovery = recover_stale_workers(
        &workspace_root,
        run_id,
        now,
        worker_stale_cutoff,
        configured_worker_max_runtime_ms(),
    );
    let failed_task_reconcile = reconcile_failed_worker_tasks(&workspace_root, run_id);
    let recovery_note = if recovery.recovered > 0 || !recovery.errors.is_empty() {
        Some(format!(
            "Recovered stale workers: {} (tasks reset: {}, dead pid: {}, kill: {}/{}, errors: {}).",
            recovery.recovered,
            recovery.reset_tasks,
            recovery.pid_dead,
            recovery.kill_succeeded,
            recovery.kill_attempted,
            recovery.errors.len()
        ))
    } else {
        None
    };
    let failed_reconcile_note =
        if failed_task_reconcile.reopened > 0 || !failed_task_reconcile.errors.is_empty() {
            Some(format!(
                "Reopened tasks from failed workers: {} of {} candidates (errors: {}).",
                failed_task_reconcile.reopened,
                failed_task_reconcile.candidates,
                failed_task_reconcile.errors.len()
            ))
        } else {
            None
        };

    // Best-effort release processing: integrates completed agent work before scheduling more.
    let release_note =
        match orchestrator::process_ready_releases(&workspace_root, Some(20), Some(run_id)) {
            Ok(summary) if summary.processed > 0 => Some(format!(
                "Processed releases: reconciled {}, merged {}, conflicts {}, failed {}.",
                summary.reconciled, summary.merged, summary.conflicts, summary.failed
            )),
            Ok(summary) if summary.reconciled > 0 => Some(format!(
                "Processed releases: reconciled {}, merged {}, conflicts {}, failed {}.",
                summary.reconciled, summary.merged, summary.conflicts, summary.failed
            )),
            Ok(_) => None,
            Err(e) => Some(format!("Release processing error: {}.", e)),
        };
    let combined_note = {
        let mut notes = Vec::new();
        if let Some(n) = release_note {
            notes.push(n);
        }
        if let Some(n) = recovery_note {
            notes.push(n);
        }
        if let Some(n) = failed_reconcile_note {
            notes.push(n);
        }
        if notes.is_empty() {
            None
        } else {
            Some(notes.join(" "))
        }
    };

    // Get project stats
    let ready_tasks = tasks::get_ready_sqlite_tasks(None).unwrap_or_default();
    let ready_count = ready_tasks.len();
    let ready_for_release = tasks::get_tasks_ready_for_release().unwrap_or_default();
    let ready_for_release_count = ready_for_release.len();

    // Active claims are heartbeat-fresh claims only.
    let active_count = count_active_claims(active_cutoff);
    let active_workers = workers::count_active_workers(run_id).unwrap_or(0);

    // Tasks in_progress without claim owner should still block orchestration.
    let orphaned_in_progress_ids: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM tasks
             WHERE status = 'in_progress'
               AND claimed_by IS NULL
               AND deleted_at IS NULL
             ORDER BY priority, created_at",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let ids = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    })
    .unwrap_or_default();
    let orphaned_in_progress_count = orphaned_in_progress_ids.len();

    // Stale claimed tasks should not consume orchestrator capacity.
    let stale_claim_ids: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM tasks
             WHERE status = 'in_progress'
               AND claimed_by IS NOT NULL
               AND COALESCE(claimed_heartbeat_at, claimed_at, 0) < ?1
               AND deleted_at IS NULL
             ORDER BY priority, created_at",
        )?;
        let rows = stmt.query_map([active_cutoff], |row| row.get(0))?;
        let ids = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    })
    .unwrap_or_default();
    let stale_claim_count = stale_claim_ids.len();

    if ready_for_release_count > 0 {
        let task_ids: Vec<_> = ready_for_release.iter().map(|t| t.id.as_str()).collect();
        let mut reason = format!(
            "{} task(s) still ready_for_release after automatic release attempt: {}. Review failures/conflicts and rerun 'bacchus process-releases' if needed.",
            ready_for_release_count,
            task_ids.join(", ")
        );
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if orphaned_in_progress_count > 0 {
        let mut reason = format!(
            "{} task(s) are in_progress without claims: {}. Reclaim with 'bacchus claim <id> <agent> --force' or reset status.",
            orphaned_in_progress_count,
            orphaned_in_progress_ids.join(", ")
        );
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if ready_count > 0 && active_count < max_concurrent as usize {
        // Ready work available and capacity to spawn.
        let slots = max_concurrent as usize - active_count;
        let to_spawn = ready_count.min(slots);
        let task_ids: Vec<_> = ready_tasks
            .iter()
            .take(to_spawn)
            .map(|t| t.id.as_str())
            .collect();

        let mut reason = if configured_orchestrator_auto_spawn() {
            if let Some(worker_cmd) = configured_worker_command() {
                let summary =
                    try_spawn_workers(&workspace_root, run_id, &ready_tasks, slots, &worker_cmd);
                let post_active_count = count_active_claims(active_cutoff);
                let post_active_workers = workers::count_active_workers(run_id).unwrap_or(0);
                let post_ready_count = tasks::get_ready_sqlite_tasks(None)
                    .unwrap_or_default()
                    .len();

                let mut msg = format!(
                    "Auto-spawn attempted {} task(s): launched {}, failed {}, backoff {}, exhausted {}. Ready remaining: {}. Active claims: {}/{}. Active workers: {}.",
                    summary.attempted,
                    summary.launched,
                    summary.failed,
                    summary.skipped_backoff,
                    summary.exhausted,
                    post_ready_count,
                    post_active_count,
                    max_concurrent,
                    post_active_workers
                );
                if !summary.errors.is_empty() {
                    msg = format!("{} Errors: {}", msg, summary.errors.join(" | "));
                }
                if summary.launched == 0 && summary.failed == 0 && summary.skipped_backoff == 0 {
                    msg = format!(
                        "{} No workers launched for candidates: {}.",
                        msg,
                        task_ids.join(", ")
                    );
                }
                msg
            } else {
                format!(
                    "Ready to spawn {} agent(s) for: {}. Active: {}/{}. Auto-spawn enabled but BACCHUS_WORKER_CMD is not set. Set it, then run 'bacchus session spawn-workers --count {}'.",
                    to_spawn,
                    task_ids.join(", "),
                    active_count,
                    max_concurrent,
                    to_spawn
                )
            }
        } else {
            format!(
                "Ready to spawn {} agent(s) for: {}. Active: {}/{}. Use 'bacchus session spawn-workers --count {}' (with BACCHUS_WORKER_CMD set) or 'bacchus claim <task_id> <agent_id>'.",
                to_spawn,
                task_ids.join(", "),
                active_count,
                max_concurrent,
                to_spawn
            )
        };

        if stale_claim_count > 0 {
            reason = format!(
                "{} Ignoring {} stale claim(s): {}. Run 'bacchus stale --cleanup' to reclaim them.",
                reason,
                stale_claim_count,
                stale_claim_ids.join(", ")
            );
        }
        if active_workers > 0 {
            reason = format!(
                "{} Worker processes currently active: {}.",
                reason, active_workers
            );
        }
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if active_count > 0 {
        // Active claims - wait for agents to complete
        let mut reason = format!(
            "Waiting for {} active agent(s) to complete. Check with 'bacchus list'.",
            active_count
        );
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if stale_claim_count > 0 {
        // No active claims but stale claimed tasks remain.
        let mut reason = format!(
            "{} stale claim(s) detected: {}. Run 'bacchus stale --cleanup' or reclaim manually.",
            stale_claim_count,
            stale_claim_ids.join(", ")
        );
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if ready_count == 0 && ready_for_release_count == 0 {
        // No ready, no in_progress, no claims - all done or all blocked
        let _ = stop_session();
        let mut reason = "All work complete or blocked. Session cleared.".to_string();
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "approve".to_string(),
            reason,
        }
    } else {
        let mut reason = "Orchestrator complete".to_string();
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "approve".to_string(),
            reason,
        }
    }
}

fn check_architect_session(session: &Session) -> HookCheckOutput {
    let agent_id = match &session.agent_id {
        Some(id) => id,
        None => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "No agent ID in architect session".to_string(),
            }
        }
    };

    // Best-effort stale message recovery so architect sessions don't deadlock on abandoned locks.
    let reclaim_note = match messages::reclaim_stale_messages() {
        Ok((requeued, failed)) if requeued > 0 || failed > 0 => Some(format!(
            "Recovered stale messages (requeued: {}, failed: {}).",
            requeued, failed
        )),
        _ => None,
    };

    // Check for pending messages for this architect
    let pending_count = with_db(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM agent_messages WHERE target_agent = ?1 AND status = 'pending'",
            [agent_id.as_str()],
            |r| r.get::<_, i32>(0),
        )
    })
    .unwrap_or(0);

    // Check for messages currently being processed
    let processing_count = with_db(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM agent_messages WHERE processing_by = ?1 AND status = 'processing'",
            [agent_id.as_str()],
            |r| r.get::<_, i32>(0),
        )
    })
    .unwrap_or(0);

    // Check for epics in planning state (architect's responsibility)
    let planning_epics = with_db(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM epics WHERE status = 'planning'",
            [],
            |r| r.get::<_, i32>(0),
        )
    })
    .unwrap_or(0);

    if processing_count > 0 {
        let mut reason = format!(
            "Architect {} has {} message(s) being processed. Complete processing before exiting.",
            agent_id, processing_count
        );
        if let Some(note) = reclaim_note.as_deref() {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if pending_count > 0 {
        let mut reason = format!(
            "Architect {} has {} pending message(s). Run 'bacchus message claim {} --limit 10', process each item, then ack with 'bacchus message ack <message_id> {}'.",
            agent_id, pending_count, agent_id, agent_id
        );
        if let Some(note) = reclaim_note.as_deref() {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if planning_epics > 0 {
        let mut reason = format!(
            "{} epic(s) in 'planning' state. Break down into tasks, then set status with 'bacchus epic set-status <epic_id> active'.",
            planning_epics
        );
        if let Some(note) = reclaim_note.as_deref() {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else {
        // No pending work - architect can exit
        let _ = stop_session();
        let mut reason = "No pending work for architect. Session cleared.".to_string();
        if let Some(note) = reclaim_note.as_deref() {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "approve".to_string(),
            reason,
        }
    }
}
