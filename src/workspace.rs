//! jj workspace management for isolated agent work
//!
//! This module manages jj workspaces for tasks, storing them in `.bacchus/workspaces/{task_id}/`.
//! Each workspace is created from the `main` bookmark.
//!
//! Key design decisions:
//! - Orchestrator-only release: Only orchestrator advances main bookmark
//! - Single-commit per task: Validated before marking ready
//! - Post-rebase commit tracking: For stuck release detection
//!
//! Override workspaces directory with BACCHUS_WORKSPACES environment variable.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use thiserror::Error;

/// Get the workspaces directory, checking BACCHUS_WORKSPACES env var first
pub fn get_workspaces_dir(workspace_root: &Path) -> PathBuf {
    match std::env::var("BACCHUS_WORKSPACES").ok().map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => workspace_root.join(path),
        None => workspace_root.join(".bacchus/workspaces"),
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub task_id: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum ReleaseResult {
    Success { commit_id: String },
    Conflicts { files: Vec<String> },
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("jj command failed: {0}")]
    JjError(String),
    #[error("Workspace already exists: {0}")]
    AlreadyExists(String),
    #[error("Workspace not found: {0}")]
    NotFound(String),
    #[error("No workspace named: {0}")]
    NoWorkspaceNamed(String),
    #[error("Workspace has conflicts: {files:?}")]
    HasConflicts { files: Vec<String> },
    #[error("Single commit required: workspace {0} has {1} commits above main")]
    MultipleCommits(String, usize),
    #[error("No commits: workspace {0} has no commits above main")]
    NoCommits(String),
    #[error("Invalid task ID format: {0}")]
    InvalidTaskId(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Validate task ID format for safe use in jj revsets
/// Must match: [A-Za-z0-9_-]+
fn validate_task_id(task_id: &str) -> Result<(), WorkspaceError> {
    if task_id.is_empty() {
        return Err(WorkspaceError::InvalidTaskId(
            "Task ID cannot be empty".to_string(),
        ));
    }

    let valid = task_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

    if !valid {
        return Err(WorkspaceError::InvalidTaskId(format!(
            "Task ID '{}' contains invalid characters. Only alphanumeric, '-', and '_' allowed.",
            task_id
        )));
    }

    Ok(())
}

/// Run a jj command and return stdout
fn run_jj(workspace_root: &Path, args: &[&str]) -> Result<String, WorkspaceError> {
    let output = Command::new("jj")
        .args(args)
        .current_dir(workspace_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorkspaceError::JjError(format!(
            "jj {} failed: {}",
            args.join(" "),
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run a jj command and return ExitStatus (for commands where non-zero might be expected)
fn run_jj_with_status(workspace_root: &Path, args: &[&str]) -> Result<ExitStatus, WorkspaceError> {
    let output = Command::new("jj")
        .args(args)
        .current_dir(workspace_root)
        .output()?;

    Ok(output.status)
}

// ============================================================================
// Workspace Creation
// ============================================================================

/// Create a new jj workspace for a task
/// Creates workspaces/{task_id} from main bookmark
pub fn create_workspace(workspace_root: &Path, task_id: &str) -> Result<WorkspaceInfo, WorkspaceError> {
    validate_task_id(task_id)?;

    let workspaces_dir = get_workspaces_dir(workspace_root);
    let workspace_path = workspaces_dir.join(task_id);

    // Ensure .bacchus/workspaces/ directory exists
    std::fs::create_dir_all(&workspaces_dir)?;

    // Check if workspace already exists
    if workspace_path.exists() {
        return Err(WorkspaceError::AlreadyExists(task_id.to_string()));
    }

    // Check if workspace is already registered in jj
    let workspace_list = run_jj(workspace_root, &["workspace", "list", "--template", "name ++ \"\\n\""])?;
    if workspace_list.lines().any(|name| name.trim() == task_id) {
        return Err(WorkspaceError::AlreadyExists(task_id.to_string()));
    }

    // Create workspace explicitly from main bookmark
    // -r main ensures we always fork from the integration point
    run_jj(
        workspace_root,
        &[
            "workspace",
            "add",
            "--name",
            task_id,
            "-r",
            "main",
            workspace_path.to_str().unwrap(),
        ],
    )?;

    Ok(WorkspaceInfo {
        task_id: task_id.to_string(),
        path: workspace_path,
    })
}

/// Remove a jj workspace (preserves commits in repo)
pub fn remove_workspace(
    workspace_root: &Path,
    task_id: &str,
    _force: bool,
) -> Result<(), WorkspaceError> {
    validate_task_id(task_id)?;

    let workspace_path = get_workspaces_dir(workspace_root).join(task_id);

    // Forget the workspace in jj (preserves commits)
    let _ = run_jj(workspace_root, &["workspace", "forget", task_id]);

    // Remove directory if it exists
    if workspace_path.exists() {
        std::fs::remove_dir_all(&workspace_path)?;
    }

    Ok(())
}

// ============================================================================
// Workspace Status & Validation
// ============================================================================

/// Check if a workspace is registered in jj (exact match)
pub fn workspace_exists(workspace_root: &Path, task_id: &str) -> Result<bool, WorkspaceError> {
    validate_task_id(task_id)?;

    let workspace_list = run_jj(workspace_root, &["workspace", "list", "--template", "name ++ \"\\n\""])?;
    Ok(workspace_list.lines().any(|name| name.trim() == task_id))
}

/// Get the working copy commit ID for a workspace
pub fn get_workspace_commit(workspace_root: &Path, task_id: &str) -> Result<String, WorkspaceError> {
    validate_task_id(task_id)?;

    let output = run_jj(
        workspace_root,
        &[
            "log",
            "--no-graph",
            "-r",
            &format!("{}@", task_id),
            "-T",
            "commit_id",
        ],
    )?;

    let commit_id = output.trim().to_string();
    if commit_id.is_empty() {
        return Err(WorkspaceError::NoWorkspaceNamed(task_id.to_string()));
    }

    Ok(commit_id)
}

/// Count commits between main and workspace tip
/// Used for single-commit validation
pub fn count_commits_above_main(workspace_root: &Path, task_id: &str) -> Result<usize, WorkspaceError> {
    validate_task_id(task_id)?;

    let output = run_jj(
        workspace_root,
        &[
            "log",
            "--no-graph",
            "-r",
            &format!("main..{}@", task_id),
            "-T",
            "commit_id\n",
        ],
    )?;

    let count = output.lines().filter(|l| !l.is_empty()).count();
    Ok(count)
}

/// Validate single-commit workflow
/// Returns the commit ID if valid, error otherwise
pub fn validate_single_commit(workspace_root: &Path, task_id: &str) -> Result<String, WorkspaceError> {
    let commit_count = count_commits_above_main(workspace_root, task_id)?;

    if commit_count == 0 {
        return Err(WorkspaceError::NoCommits(task_id.to_string()));
    }

    if commit_count > 1 {
        return Err(WorkspaceError::MultipleCommits(task_id.to_string(), commit_count));
    }

    // Get the commit ID
    get_workspace_commit(workspace_root, task_id)
}

// ============================================================================
// Conflict Detection
// ============================================================================

/// Check if workspace has conflicts
pub fn has_conflicts(workspace_root: &Path, task_id: &str) -> Result<bool, WorkspaceError> {
    validate_task_id(task_id)?;

    let output = run_jj(
        workspace_root,
        &[
            "log",
            "--no-graph",
            "-r",
            &format!("{}@", task_id),
            "-T",
            "conflict",
        ],
    )?;

    Ok(output.trim() == "true")
}

/// Get list of conflicted files
pub fn get_conflict_files(workspace_root: &Path, task_id: &str) -> Result<Vec<String>, WorkspaceError> {
    validate_task_id(task_id)?;

    // jj resolve --list shows files with conflicts
    let output = run_jj(
        workspace_root,
        &["resolve", "--list", "-r", &format!("{}@", task_id)],
    )?;

    Ok(output.lines().map(String::from).collect())
}

// ============================================================================
// Release Operations (Orchestrator Only)
// ============================================================================

/// Rebase workspace onto main and return result
/// Does NOT advance main bookmark - that's done separately
pub fn rebase_workspace_onto_main(
    workspace_root: &Path,
    task_id: &str,
) -> Result<ReleaseResult, WorkspaceError> {
    validate_task_id(task_id)?;

    // Rebase workspace working copy onto main
    let rebase_status = run_jj_with_status(
        workspace_root,
        &["rebase", "-r", &format!("{}@", task_id), "-d", "main"],
    )?;

    // Check for conflicts (jj may return 0 even with conflicts)
    if has_conflicts(workspace_root, task_id)? {
        let files = get_conflict_files(workspace_root, task_id)?;
        return Ok(ReleaseResult::Conflicts { files });
    }

    // If rebase failed and no conflicts, it's a hard error
    if !rebase_status.success() {
        return Err(WorkspaceError::JjError(format!(
            "jj rebase failed with exit code: {:?}",
            rebase_status.code()
        )));
    }

    // Get the NEW commit ID after rebase
    let new_commit_id = get_workspace_commit(workspace_root, task_id)?;

    Ok(ReleaseResult::Success {
        commit_id: new_commit_id,
    })
}

/// Advance main bookmark to a commit
pub fn advance_main_bookmark(workspace_root: &Path, commit_id: &str) -> Result<(), WorkspaceError> {
    run_jj(workspace_root, &["bookmark", "set", "main", "-r", commit_id])?;
    Ok(())
}

/// Complete release: forget workspace and clean up directory
pub fn complete_release(workspace_root: &Path, task_id: &str) -> Result<(), WorkspaceError> {
    validate_task_id(task_id)?;

    // Forget the workspace (preserves commits in repo)
    let _ = run_jj(workspace_root, &["workspace", "forget", task_id]);

    // Clean up workspace directory
    let workspace_path = get_workspaces_dir(workspace_root).join(task_id);
    if workspace_path.exists() {
        std::fs::remove_dir_all(&workspace_path)?;
    }

    Ok(())
}

// ============================================================================
// Ancestry Checks (for stuck release detection)
// ============================================================================

/// Check if a commit is reachable from main (i.e., is an ancestor)
pub fn is_commit_in_main(workspace_root: &Path, commit_id: &str) -> Result<bool, WorkspaceError> {
    // Check if commit is in ancestors of main
    let output = run_jj(
        workspace_root,
        &[
            "log",
            "-r",
            &format!("{} & ::main", commit_id),
            "--no-graph",
            "-T",
            "commit_id",
        ],
    )
    .unwrap_or_default();

    // If output contains our commit ID, it's reachable from main
    let trimmed = output.trim();
    Ok(!trimmed.is_empty() && trimmed.contains(commit_id))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_task_id_valid() {
        assert!(validate_task_id("AUTH-001").is_ok());
        assert!(validate_task_id("test_task").is_ok());
        assert!(validate_task_id("Task123").is_ok());
        assert!(validate_task_id("a").is_ok());
    }

    #[test]
    fn test_validate_task_id_invalid() {
        assert!(validate_task_id("").is_err());
        assert!(validate_task_id("task with space").is_err());
        assert!(validate_task_id("task@special").is_err());
        assert!(validate_task_id("task::nested").is_err());
        assert!(validate_task_id("task/path").is_err());
    }

    #[test]
    fn test_get_workspaces_dir_default() {
        let root = PathBuf::from("/test/repo");
        let dir = get_workspaces_dir(&root);
        assert_eq!(dir, PathBuf::from("/test/repo/.bacchus/workspaces"));
    }
}
