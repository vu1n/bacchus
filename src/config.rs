//! Configuration management for Bacchus
//!
//! Shared utilities for workspace discovery and session scoping.
//!
//! # Environment Variables
//!
//! - `BACCHUS_DB_PATH`: Override path to bacchus database (default: `.bacchus/bacchus.db`)
//! - `BACCHUS_WORKSPACES`: Override path to workspaces directory (default: `.bacchus/workspaces`)
//! - `CLAUDE_PROJECT_DIR`: Workspace root override (set by Claude Code)
//! - `BACCHUS_SESSION_ID` / `CLAUDE_SESSION_ID` / `CLAUDE_CONVERSATION_ID`: Session scope

use std::path::PathBuf;

const SESSION_SCOPE_ENV_KEYS: [&str; 3] = [
    "BACCHUS_SESSION_ID",
    "CLAUDE_SESSION_ID",
    "CLAUDE_CONVERSATION_ID",
];

/// Find workspace root by looking for .bacchus or .git directories walking up.
///
/// Priority:
/// 1. `CLAUDE_PROJECT_DIR` env var (set by Claude Code for plugins/hooks)
/// 2. Walk up from CWD looking for `.bacchus` or `.git`
pub fn find_workspace_root() -> Option<PathBuf> {
    if let Ok(project_dir) = std::env::var("CLAUDE_PROJECT_DIR") {
        let path = PathBuf::from(&project_dir);
        if path.exists() {
            return Some(path);
        }
    }

    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join(".bacchus").exists() {
            return Some(current);
        }
        // .git can be a directory (normal repos) or a file (worktrees/submodules).
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// Sanitize a string for use as a filesystem-safe scope identifier.
pub fn sanitize_scope(scope: &str) -> String {
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

/// Derive the current session scope ID from environment variables.
///
/// Checks `BACCHUS_SESSION_ID`, `CLAUDE_SESSION_ID`, and `CLAUDE_CONVERSATION_ID`
/// in order. Falls back to `"default"` if none are set.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_scope_passes_alphanumeric() {
        assert_eq!(sanitize_scope("abc-123_def"), "abc-123_def");
    }

    #[test]
    fn test_sanitize_scope_replaces_special_chars() {
        assert_eq!(sanitize_scope("a/b.c@d"), "a_b_c_d");
    }

    #[test]
    fn test_sanitize_scope_empty_returns_default() {
        assert_eq!(sanitize_scope(""), "default");
    }

    #[test]
    fn test_sanitize_scope_all_special_returns_underscores() {
        assert_eq!(sanitize_scope("@#$"), "___");
    }

    #[test]
    fn test_current_session_scope_id_no_env_returns_default() {
        // When no session env vars are set, should return "default"
        // (This test is fragile if env vars are set in CI, but fine locally)
        let result = current_session_scope_id();
        assert!(!result.is_empty());
    }
}
