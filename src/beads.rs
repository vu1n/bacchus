//! Beads integration module
//!
//! Provides functions to interact with the beads issue tracking system via the `bd` CLI.
//! This ensures we use bd's business logic, views, and sync mechanisms.

use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;

// ============================================================================
// Types
// ============================================================================

/// Information about a bead (issue) - our internal representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeadInfo {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: i32,
    pub status: String,
}

/// Raw issue format from `bd --json` output
#[derive(Debug, Deserialize)]
struct BdIssue {
    id: String,
    title: String,
    description: Option<String>,
    status: String,
    priority: i32,
    // Other fields we don't need: issue_type, created_at, updated_at, etc.
}

impl From<BdIssue> for BeadInfo {
    fn from(issue: BdIssue) -> Self {
        BeadInfo {
            id: issue.id,
            title: issue.title,
            description: issue.description,
            priority: issue.priority,
            status: issue.status,
        }
    }
}

/// Errors that can occur when interacting with beads
#[derive(Debug, Error)]
pub enum BeadsError {
    #[error("bd command failed: {0}")]
    CommandFailed(String),

    #[error("bd not found - is it installed?")]
    BdNotFound,

    #[error("Failed to parse bd output: {0}")]
    ParseError(String),

    #[error("Bead not found: {0}")]
    BeadNotFound(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

// ============================================================================
// Public API
// ============================================================================

/// Find beads that are ready to work on (via `bd ready --json`)
pub fn get_ready_beads() -> Result<Vec<BeadInfo>, BeadsError> {
    let output = Command::new("bd")
        .args(["ready", "--json", "--quiet"])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                BeadsError::BdNotFound
            } else {
                BeadsError::IoError(e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BeadsError::CommandFailed(stderr.to_string()));
    }

    let issues: Vec<BdIssue> = serde_json::from_slice(&output.stdout)
        .map_err(|e| BeadsError::ParseError(e.to_string()))?;

    Ok(issues.into_iter().map(BeadInfo::from).collect())
}

/// Update a bead's status (via `bd update <id> --status <status>`)
pub fn update_bead_status(bead_id: &str, status: &str) -> Result<(), BeadsError> {
    let output = Command::new("bd")
        .args(["update", bead_id, "--status", status, "--quiet"])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                BeadsError::BdNotFound
            } else {
                BeadsError::IoError(e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found") || stderr.contains("no issue") {
            return Err(BeadsError::BeadNotFound(bead_id.to_string()));
        }
        return Err(BeadsError::CommandFailed(stderr.to_string()));
    }

    Ok(())
}

/// Get details for a specific bead (via `bd show <id> --json`)
pub fn get_bead(bead_id: &str) -> Result<BeadInfo, BeadsError> {
    let output = Command::new("bd")
        .args(["show", bead_id, "--json", "--quiet"])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                BeadsError::BdNotFound
            } else {
                BeadsError::IoError(e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found") || stderr.contains("no issue") {
            return Err(BeadsError::BeadNotFound(bead_id.to_string()));
        }
        return Err(BeadsError::CommandFailed(stderr.to_string()));
    }

    // bd show returns a single object, not an array
    let issue: BdIssue = serde_json::from_slice(&output.stdout)
        .map_err(|e| BeadsError::ParseError(e.to_string()))?;

    Ok(BeadInfo::from(issue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bd_issue_to_bead_info() {
        let bd_issue = BdIssue {
            id: "TEST-1".to_string(),
            title: "Test issue".to_string(),
            description: Some("A description".to_string()),
            status: "open".to_string(),
            priority: 1,
        };

        let bead: BeadInfo = bd_issue.into();
        assert_eq!(bead.id, "TEST-1");
        assert_eq!(bead.title, "Test issue");
        assert_eq!(bead.priority, 1);
        assert_eq!(bead.status, "open");
    }
}
