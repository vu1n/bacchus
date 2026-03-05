//! Heartbeat and lease loops: background processes for session liveness.

use crate::tasks;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

/// Check if a specific PID is still alive.
/// Uses `kill -0` which checks process existence without sending a signal.
fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(true) // assume alive on error
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Get the current process's parent PID.
fn get_ppid() -> Option<u32> {
    std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
}

use super::config::*;
use super::file::*;
use super::types::SessionMode;
use super::workers::generate_run_id;

pub(super) fn spawn_agent_heartbeat_loop(
    task_id: &str,
    agent_id: &str,
    token: &str,
    interval_ms: u64,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("session")
        .arg("heartbeat-loop")
        .arg("--task-id")
        .arg(task_id)
        .arg("--agent-id")
        .arg(agent_id)
        .arg("--token")
        .arg(token)
        .arg("--interval-ms")
        .arg(interval_ms.to_string());
    // Pass our parent PID (the Claude Code process) so the loop can detect orphaning.
    if let Some(ppid) = get_ppid() {
        cmd.arg("--owner-pid").arg(ppid.to_string());
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn spawn_orchestrator_lease_loop(
    run_id: &str,
    token: &str,
    interval_ms: u64,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("session")
        .arg("lease-loop")
        .arg("--run-id")
        .arg(run_id)
        .arg("--token")
        .arg(token)
        .arg("--interval-ms")
        .arg(interval_ms.to_string());
    // Pass our parent PID (the Claude Code process) so the loop can detect orphaning.
    if let Some(ppid) = get_ppid() {
        cmd.arg("--owner-pid").arg(ppid.to_string());
    }
    cmd.stdin(Stdio::null())
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
    if session.mode != SessionMode::Agent || session.task_id.as_deref() != Some(task_id) {
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
    owner_pid: Option<u32>,
) -> Result<String, String> {
    let interval = Duration::from_millis(interval_ms.max(100));

    loop {
        if let Some(pid) = owner_pid {
            if !is_pid_alive(pid) {
                break;
            }
        }

        let path = match session_path() {
            Some(p) if p.exists() => p,
            _ => break,
        };

        let session = match read_session(&path) {
            Ok(s) => s,
            Err(_) => break,
        };

        // Exit if this loop is no longer the session's active heartbeat owner.
        if session.mode != SessionMode::Agent
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
    owner_pid: Option<u32>,
) -> Result<String, String> {
    let interval = Duration::from_millis(interval_ms.max(100));
    let ttl_ms = configured_orchestrator_lease_ttl_ms();

    loop {
        if let Some(pid) = owner_pid {
            if !is_pid_alive(pid) {
                break;
            }
        }

        let path = match session_path() {
            Some(p) if p.exists() => p,
            _ => break,
        };

        let session = match read_session(&path) {
            Ok(s) => s,
            Err(_) => break,
        };

        if session.mode != SessionMode::Orchestrator
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
