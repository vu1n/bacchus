//! Session management for stop hooks
//!
//! Manages scoped session files under .bacchus/sessions/ for persistent session state.

use crate::db::with_db;
use crate::handles;
use crate::tasks;
use crate::tools::orchestrator;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const DEFAULT_AGENT_HEARTBEAT_INTERVAL_MS: u64 = 30_000;
const DEFAULT_ORCHESTRATOR_LEASE_RENEW_INTERVAL_MS: u64 = 30_000;
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
        return Ok(serde_json::json!({
            "active": true,
            "session": session,
            "scope_id": current_session_scope_id(),
            "path": path.to_string_lossy()
        }));
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
                _ => {
                    if let Some(session_agent_id) = session.agent_id.as_deref() {
                        if task.claimed_by.as_deref() == Some(session_agent_id) {
                            let _ = tasks::heartbeat_sqlite_task(task_id, session_agent_id);
                        }
                    }
                    HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} status is '{}'. Continue working until complete, then run 'bacchus release {} --status done' or '--status blocked'.",
                        task_id,
                        task.status.as_str(),
                        task_id
                    ),
                }
                }
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

    // Get project stats
    let ready_tasks = tasks::get_ready_sqlite_tasks(None).unwrap_or_default();
    let ready_count = ready_tasks.len();
    let ready_for_release = tasks::get_tasks_ready_for_release().unwrap_or_default();
    let ready_for_release_count = ready_for_release.len();

    // Active claims are heartbeat-fresh claims only.
    let active_count = with_db(|conn| {
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
    .unwrap_or(0) as usize;

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
        if let Some(note) = &release_note {
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
        if let Some(note) = &release_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if ready_count > 0 && active_count < max_concurrent as usize {
        // Ready work available and capacity to spawn
        let slots = max_concurrent as usize - active_count;
        let to_spawn = ready_count.min(slots);
        let task_ids: Vec<_> = ready_tasks
            .iter()
            .take(to_spawn)
            .map(|t| t.id.as_str())
            .collect();

        let mut reason = format!(
            "Ready to spawn {} agent(s) for: {}. Active: {}/{}. Use 'bacchus claim <task_id> <agent_id>' to claim.",
            to_spawn,
            task_ids.join(", "),
            active_count,
            max_concurrent
        );
        if stale_claim_count > 0 {
            reason = format!(
                "{} Ignoring {} stale claim(s): {}. Run 'bacchus stale --cleanup' to reclaim them.",
                reason,
                stale_claim_count,
                stale_claim_ids.join(", ")
            );
        }
        if let Some(note) = &release_note {
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
        if let Some(note) = &release_note {
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
        if let Some(note) = &release_note {
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
        if let Some(note) = &release_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "approve".to_string(),
            reason,
        }
    } else {
        let mut reason = "Orchestrator complete".to_string();
        if let Some(note) = &release_note {
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
        HookCheckOutput {
            decision: "block".to_string(),
            reason: format!(
                "Architect {} has {} message(s) being processed. Complete processing before exiting.",
                agent_id, processing_count
            ),
        }
    } else if pending_count > 0 {
        HookCheckOutput {
            decision: "block".to_string(),
            reason: format!(
                "Architect {} has {} pending message(s). Poll and process messages before exiting.",
                agent_id, pending_count
            ),
        }
    } else if planning_epics > 0 {
        HookCheckOutput {
            decision: "block".to_string(),
            reason: format!(
                "{} epic(s) in 'planning' state. Break down into tasks before exiting.",
                planning_epics
            ),
        }
    } else {
        // No pending work - architect can exit
        let _ = stop_session();
        HookCheckOutput {
            decision: "approve".to_string(),
            reason: "No pending work for architect. Session cleared.".to_string(),
        }
    }
}
