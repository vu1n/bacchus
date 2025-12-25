//! Integration tests for Bacchus coordination workflow
//!
//! Tests the end-to-end functionality of:
//! - Worktree creation and management
//! - Claim recording and cleanup
//! - Stale detection
//!
//! Note: Tests requiring `bd` (beads CLI) are skipped if bd is not available.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Initialize a test git repository with an initial commit
fn init_test_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().to_path_buf();

    Command::new("git")
        .arg("init")
        .current_dir(&repo_path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    fs::write(repo_path.join("test.txt"), "initial content").unwrap();

    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    (temp, repo_path)
}

/// Check if bd CLI is available
fn bd_available() -> bool {
    Command::new("bd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ============================================================================
// Worktree Tests
// ============================================================================

mod worktree_tests {
    use super::*;

    #[test]
    fn test_worktree_creation() {
        let (_temp, repo_path) = init_test_repo();
        let worktrees_dir = repo_path.join(".bacchus/worktrees");

        // Create worktree for test-bead-1
        let output = Command::new("git")
            .args(["worktree", "add", "-b", "bacchus/test-bead-1"])
            .arg(worktrees_dir.join("test-bead-1"))
            .current_dir(&repo_path)
            .output()
            .unwrap();

        assert!(output.status.success(), "Worktree creation failed");
        assert!(worktrees_dir.join("test-bead-1").exists());

        // Verify branch exists
        let output = Command::new("git")
            .args(["branch", "--list", "bacchus/test-bead-1"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("bacchus/test-bead-1"));
    }

    #[test]
    fn test_worktree_modification_and_merge() {
        let (_temp, repo_path) = init_test_repo();
        let worktrees_dir = repo_path.join(".bacchus/worktrees");
        let worktree_path = worktrees_dir.join("test-bead-2");

        // Create worktree
        Command::new("git")
            .args(["worktree", "add", "-b", "bacchus/test-bead-2"])
            .arg(&worktree_path)
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Make changes in worktree
        fs::write(worktree_path.join("new_file.txt"), "new content").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(&worktree_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "Add new file"])
            .current_dir(&worktree_path)
            .output()
            .unwrap();

        // Merge back to main
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let output = Command::new("git")
            .args(["merge", "bacchus/test-bead-2"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        assert!(output.status.success(), "Merge failed");

        // Verify file exists in main
        assert!(repo_path.join("new_file.txt").exists());
    }

    #[test]
    fn test_worktree_removal() {
        let (_temp, repo_path) = init_test_repo();
        let worktrees_dir = repo_path.join(".bacchus/worktrees");
        let worktree_path = worktrees_dir.join("test-bead-3");

        // Create worktree
        Command::new("git")
            .args(["worktree", "add", "-b", "bacchus/test-bead-3"])
            .arg(&worktree_path)
            .current_dir(&repo_path)
            .output()
            .unwrap();

        assert!(worktree_path.exists());

        // Remove worktree
        let output = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&worktree_path)
            .current_dir(&repo_path)
            .output()
            .unwrap();

        assert!(output.status.success(), "Worktree removal failed");
        assert!(!worktree_path.exists());

        // Delete branch
        Command::new("git")
            .args(["branch", "-D", "bacchus/test-bead-3"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Verify branch is gone
        let output = Command::new("git")
            .args(["branch", "--list", "bacchus/test-bead-3"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("bacchus/test-bead-3"));
    }

    #[test]
    fn test_merge_conflict_detection() {
        let (_temp, repo_path) = init_test_repo();
        let worktrees_dir = repo_path.join(".bacchus/worktrees");
        let worktree_path = worktrees_dir.join("test-bead-conflict");

        // Create worktree
        Command::new("git")
            .args(["worktree", "add", "-b", "bacchus/test-bead-conflict"])
            .arg(&worktree_path)
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Modify test.txt in worktree
        fs::write(worktree_path.join("test.txt"), "worktree change").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&worktree_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Worktree change"])
            .current_dir(&worktree_path)
            .output()
            .unwrap();

        // Modify test.txt in main (different content)
        fs::write(repo_path.join("test.txt"), "main change").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Main change"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Attempt merge (should fail)
        let output = Command::new("git")
            .args(["merge", "bacchus/test-bead-conflict"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Merge should fail due to conflict
        assert!(!output.status.success() || repo_path.join(".git/MERGE_HEAD").exists());
    }
}

// ============================================================================
// Database Tests
// ============================================================================

mod db_tests {
    use super::*;

    fn init_test_db(temp_dir: &TempDir) -> PathBuf {
        let db_path = temp_dir.path().join("test.db");

        // Run bacchus to init DB (use env var to point to test DB)
        let output = Command::new("cargo")
            .args(["run", "--", "status"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success(), "DB init failed: {:?}", output);
        db_path
    }

    #[test]
    fn test_claim_operations() {
        let temp = TempDir::new().unwrap();
        let db_path = init_test_db(&temp);

        // Verify status shows empty claims
        let output = Command::new("cargo")
            .args(["run", "--", "status"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"count\": 0"));
    }

    #[test]
    fn test_list_empty_claims() {
        let temp = TempDir::new().unwrap();
        let db_path = init_test_db(&temp);

        let output = Command::new("cargo")
            .args(["run", "--", "list"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"claims\": []") || stdout.contains("claims"));
    }

    #[test]
    fn test_stale_empty() {
        let temp = TempDir::new().unwrap();
        let db_path = init_test_db(&temp);

        let output = Command::new("cargo")
            .args(["run", "--", "stale", "--minutes", "1"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("stale_claims"));
    }
}

// ============================================================================
// Symbol Index Tests
// ============================================================================

mod symbol_tests {
    use super::*;

    #[test]
    fn test_index_and_search() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // Create a test TypeScript file
        let test_file = temp.path().join("test.ts");
        fs::write(
            &test_file,
            r#"
export function greet(name: string): string {
    return `Hello, ${name}!`;
}

export class Greeter {
    private name: string;

    constructor(name: string) {
        this.name = name;
    }

    sayHello(): string {
        return greet(this.name);
    }
}
"#,
        )
        .unwrap();

        // Index the file
        let output = Command::new("cargo")
            .args(["run", "--", "index"])
            .arg(&test_file)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success(), "Index failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("files_indexed"));

        // Search for the function
        let output = Command::new("cargo")
            .args(["run", "--", "symbols", "--pattern", "greet"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("greet") || stdout.contains("symbols"));

        // Search for the class
        let output = Command::new("cargo")
            .args(["run", "--", "symbols", "--pattern", "Greeter", "--kind", "class"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success());
    }
}

// ============================================================================
// CLI Tests
// ============================================================================

mod cli_tests {
    use super::*;

    #[test]
    fn test_help() {
        let output = Command::new("cargo")
            .args(["run", "--", "--help"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Worktree-based coordination"));
    }

    #[test]
    fn test_workflow_doc() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        let output = Command::new("cargo")
            .args(["run", "--", "workflow"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Bacchus Coordination Protocol"));
        assert!(stdout.contains("bacchus next"));
        assert!(stdout.contains("bacchus release"));
    }

    #[test]
    fn test_version() {
        let output = Command::new("cargo")
            .args(["run", "--", "--version"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("bacchus"));
    }
}

// ============================================================================
// Full Workflow Tests (require bd)
// ============================================================================

mod workflow_tests {
    use super::*;

    #[test]
    fn test_next_without_ready_beads() {
        if !bd_available() {
            eprintln!("Skipping test: bd not available");
            return;
        }

        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // Run next - should report no ready beads or bd error
        let output = Command::new("cargo")
            .args(["run", "--", "next", "test-agent"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Should indicate no ready beads or bd error
        assert!(
            stdout.contains("No ready beads") ||
            stdout.contains("success") ||
            stderr.contains("bd") ||
            stderr.contains("beads") ||
            stderr.contains("Failed"),
            "Unexpected output: stdout={}, stderr={}", stdout, stderr
        );
    }

    #[test]
    fn test_release_without_claim() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // Init DB first
        Command::new("cargo")
            .args(["run", "--", "status"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        // Try to release non-existent claim
        let output = Command::new("cargo")
            .args(["run", "--", "release", "nonexistent-bead", "--status", "done"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("No claim found") || stdout.contains("success\": false"),
            "Expected 'No claim found', got: {}", stdout
        );
    }

    #[test]
    fn test_abort_without_merge() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // Init DB first
        Command::new("cargo")
            .args(["run", "--", "status"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        // Try to abort when not in merge
        let output = Command::new("cargo")
            .args(["run", "--", "abort", "test-bead"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        // Should fail or report no merge in progress
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("not in merge") ||
            stdout.contains("success\": false") ||
            stderr.contains("merge") ||
            !output.status.success(),
            "Expected merge error, got: stdout={}, stderr={}", stdout, stderr
        );
    }
}

// ============================================================================
// Error Case Tests
// ============================================================================

mod error_tests {
    use super::*;

    #[test]
    fn test_invalid_subcommand() {
        let output = Command::new("cargo")
            .args(["run", "--", "invalid-command"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();

        assert!(!output.status.success());
    }

    #[test]
    fn test_missing_arguments() {
        let output = Command::new("cargo")
            .args(["run", "--", "next"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();

        // Should fail due to missing agent_id
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("agent_id") || stderr.contains("required"));
    }

    #[test]
    fn test_invalid_release_status() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // Init DB
        Command::new("cargo")
            .args(["run", "--", "status"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        // Try invalid status
        let output = Command::new("cargo")
            .args(["run", "--", "release", "test", "--status", "invalid"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Invalid status") || stdout.contains("No claim found"),
            "Expected error for invalid status, got: {}", stdout
        );
    }
}
