//! Session file I/O: paths, reading, writing, and age calculation.

use super::types::Session;
use crate::config::{current_session_scope_id, find_workspace_root, sanitize_scope};
use std::fs;
use std::path::Path;

pub(super) fn session_file_path_for_scope(scope: &str) -> Option<std::path::PathBuf> {
    find_workspace_root().map(|root| {
        root.join(".bacchus")
            .join("sessions")
            .join(format!("{}.json", sanitize_scope(scope)))
    })
}

pub(super) fn scoped_session_path() -> Option<std::path::PathBuf> {
    session_file_path_for_scope(&current_session_scope_id())
}

pub(super) fn legacy_session_path() -> Option<std::path::PathBuf> {
    find_workspace_root().map(|root| root.join(".bacchus/session.json"))
}

pub(super) fn session_path() -> Option<std::path::PathBuf> {
    if let Some(path) = scoped_session_path() {
        if path.exists() {
            return Some(path);
        }
    }
    legacy_session_path().filter(|p| p.exists())
}

pub(super) fn read_session(path: &Path) -> Result<Session, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content).map_err(|e| e.to_string())
}

pub(super) fn write_session(path: &Path, session: &Session) -> Result<(), String> {
    let json = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
}

pub(super) fn session_age_minutes(session: &Session, path: &Path) -> Option<i64> {
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

pub(super) fn sessions_dir() -> Option<std::path::PathBuf> {
    find_workspace_root().map(|root| root.join(".bacchus").join("sessions"))
}

/// Get the current process's parent PID.
pub(super) fn get_ppid() -> Option<u32> {
    std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
}

pub(super) fn same_default_scope_identity(existing: &Session, new_session: &Session) -> bool {
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
