//! Session types and data structures.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// Claim and work on a single task
    Agent,
    /// Manage workers and process releases
    Orchestrator,
    /// Break down epics into tasks
    Architect,
}

impl std::fmt::Display for SessionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionMode::Agent => write!(f, "agent"),
            SessionMode::Orchestrator => write!(f, "orchestrator"),
            SessionMode::Architect => write!(f, "architect"),
        }
    }
}

/// Session state stored in scoped .bacchus/sessions/<scope>.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub mode: SessionMode,
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
