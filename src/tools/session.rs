//! Session management for stop hooks
//!
//! Manages .bacchus/session.json for persistent session state.

use crate::beads;
use crate::db::with_db;
use serde::{Deserialize, Serialize};
use std::fs;

/// Session state stored in .bacchus/session.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bead_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<i32>,
    pub started_at: String,
}

/// Output for hook check command
#[derive(Debug, Serialize, Deserialize)]
pub struct HookCheckOutput {
    pub decision: String, // "approve" or "block"
    pub reason: String,
}

/// Find workspace root (where .bacchus or .beads exists)
fn find_workspace_root() -> Option<std::path::PathBuf> {
    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join(".bacchus").exists() || current.join(".beads").exists() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

fn session_path() -> Option<std::path::PathBuf> {
    find_workspace_root().map(|root| root.join(".bacchus/session.json"))
}

/// Start a session
pub fn start_session(mode: &str, bead_id: Option<&str>, max_concurrent: i32) -> Result<String, String> {
    let root = find_workspace_root().ok_or("No workspace root found")?;
    let bacchus_dir = root.join(".bacchus");
    fs::create_dir_all(&bacchus_dir).map_err(|e| e.to_string())?;

    let session = match mode {
        "agent" => {
            let bead_id = bead_id.ok_or("bead_id required for agent mode")?;
            Session {
                mode: "agent".to_string(),
                bead_id: Some(bead_id.to_string()),
                max_concurrent: None,
                started_at: chrono::Utc::now().to_rfc3339(),
            }
        }
        "orchestrator" => Session {
            mode: "orchestrator".to_string(),
            bead_id: None,
            max_concurrent: Some(max_concurrent),
            started_at: chrono::Utc::now().to_rfc3339(),
        },
        _ => return Err(format!("Unknown mode: {}. Use 'agent' or 'orchestrator'", mode)),
    };

    let json = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;
    fs::write(bacchus_dir.join("session.json"), &json).map_err(|e| e.to_string())?;

    Ok(format!("Started {} session", mode))
}

/// Stop the session
pub fn stop_session() -> Result<String, String> {
    if let Some(path) = session_path() {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
            return Ok("Session stopped".to_string());
        }
    }
    Ok("No active session".to_string())
}

/// Get current session status
pub fn session_status() -> Result<serde_json::Value, String> {
    if let Some(path) = session_path() {
        if path.exists() {
            let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let session: Session = serde_json::from_str(&content).map_err(|e| e.to_string())?;
            return Ok(serde_json::json!({
                "active": true,
                "session": session,
                "path": path.to_string_lossy()
            }));
        }
    }
    Ok(serde_json::json!({
        "active": false,
        "session": null
    }))
}

/// Check if session should block exit (for stop hook)
pub fn check_session() -> HookCheckOutput {
    // Read session file
    let session = match session_path() {
        Some(path) if path.exists() => {
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<Session>(&content) {
                    Ok(s) => s,
                    Err(_) => return HookCheckOutput {
                        decision: "approve".to_string(),
                        reason: "Invalid session file".to_string(),
                    },
                },
                Err(_) => return HookCheckOutput {
                    decision: "approve".to_string(),
                    reason: "Cannot read session file".to_string(),
                },
            }
        }
        _ => return HookCheckOutput {
            decision: "approve".to_string(),
            reason: "No bacchus session active".to_string(),
        },
    };

    match session.mode.as_str() {
        "agent" => check_agent_session(&session),
        "orchestrator" => check_orchestrator_session(&session),
        _ => HookCheckOutput {
            decision: "approve".to_string(),
            reason: format!("Unknown session mode: {}", session.mode),
        },
    }
}

fn check_agent_session(session: &Session) -> HookCheckOutput {
    let bead_id = match &session.bead_id {
        Some(id) => id,
        None => return HookCheckOutput {
            decision: "approve".to_string(),
            reason: "No bead ID in session".to_string(),
        },
    };

    // Check bead status
    match beads::get_bead(bead_id) {
        Ok(bead) => {
            if bead.status == "closed" {
                // Auto-clear session
                let _ = stop_session();
                HookCheckOutput {
                    decision: "approve".to_string(),
                    reason: format!("Bead {} is closed. Session cleared.", bead_id),
                }
            } else {
                HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Bead {} status is '{}'. Continue working until complete, then run 'bd close {}'.",
                        bead_id, bead.status, bead_id
                    ),
                }
            }
        }
        Err(e) => HookCheckOutput {
            decision: "approve".to_string(),
            reason: format!("Cannot check bead status: {}", e),
        },
    }
}

fn check_orchestrator_session(session: &Session) -> HookCheckOutput {
    let max_concurrent = session.max_concurrent.unwrap_or(3);

    // Get project stats
    let ready_beads = beads::get_ready_beads().unwrap_or_default();
    let ready_count = ready_beads.len();

    // Get active claims count
    let active_count = with_db(|conn| {
        conn.query_row("SELECT COUNT(*) FROM claims", [], |r| r.get::<_, i32>(0))
    })
    .unwrap_or(0) as usize;

    // Get overall stats by checking all beads
    // For simplicity, we'll use the ready count as an indicator
    // A more complete implementation would query bd status

    if ready_count > 0 && active_count < max_concurrent as usize {
        let slots = max_concurrent as usize - active_count;
        let to_spawn = ready_count.min(slots);
        let bead_ids: Vec<_> = ready_beads.iter().take(to_spawn).map(|b| b.id.as_str()).collect();

        HookCheckOutput {
            decision: "block".to_string(),
            reason: format!(
                "Ready to spawn {} agent(s) for: {}. Active: {}/{}. Use 'bacchus claim <bead_id> <agent_id>' to claim.",
                to_spawn,
                bead_ids.join(", "),
                active_count,
                max_concurrent
            ),
        }
    } else if active_count > 0 {
        HookCheckOutput {
            decision: "block".to_string(),
            reason: format!(
                "Waiting for {} active agent(s) to complete. Check with 'bacchus list'.",
                active_count
            ),
        }
    } else if ready_count == 0 {
        // No ready beads and no active agents - either all done or all blocked
        let _ = stop_session();
        HookCheckOutput {
            decision: "approve".to_string(),
            reason: "All work complete or blocked. Session cleared.".to_string(),
        }
    } else {
        HookCheckOutput {
            decision: "approve".to_string(),
            reason: "Orchestrator complete".to_string(),
        }
    }
}
