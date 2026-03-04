//! Integration tests for Bacchus coordination workflow
//!
//! Tests the end-to-end functionality of:
//! - jj workspace creation and management
//! - Claim recording and cleanup
//! - Stale detection
//!
//! Note: Tests run against the pre-built binary to avoid Cargo lock contention.
//! Run `cargo build` before running integration tests.

use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Get the path to the bacchus binary (pre-built to avoid Cargo lock contention)
fn bacchus_bin() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("target/debug/bacchus")
}

/// Check if jj is installed
fn jj_installed() -> bool {
    Command::new("jj")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Initialize a test jj repository with an initial commit
fn init_test_repo() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let repo_path = temp.path().to_path_buf();

    // Initialize jj repo with git backend (colocated)
    Command::new("jj")
        .args(["git", "init", "--colocate"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    // Configure user
    Command::new("jj")
        .args(["config", "set", "--repo", "user.name", "Test"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    Command::new("jj")
        .args(["config", "set", "--repo", "user.email", "test@test.com"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    // Create initial file
    fs::write(repo_path.join("test.txt"), "initial content").unwrap();

    // Create bookmark for main (jj uses bookmarks, not branches)
    Command::new("jj")
        .args(["bookmark", "create", "main", "-r", "@"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    // Describe the initial commit
    Command::new("jj")
        .args(["describe", "-m", "Initial commit"])
        .current_dir(&repo_path)
        .output()
        .unwrap();

    // Create a new empty working copy change (so main points to a real commit)
    Command::new("jj")
        .arg("new")
        .current_dir(&repo_path)
        .output()
        .unwrap();

    (temp, repo_path)
}

// ============================================================================
// Workspace Tests (jj)
// ============================================================================

mod workspace_tests {
    use super::*;

    #[test]
    fn test_workspace_creation() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let workspaces_dir = repo_path.join(".bacchus/workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let workspace_path = workspaces_dir.join("test-task-1");

        // Create jj workspace for test-task-1
        let output = Command::new("jj")
            .args(["workspace", "add", "--name", "test-task-1"])
            .arg(&workspace_path)
            .current_dir(&repo_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "Workspace creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(workspace_path.exists());

        // Verify workspace exists in jj workspace list
        let output = Command::new("jj")
            .args(["workspace", "list"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("test-task-1"));
    }

    #[test]
    fn test_workspace_modification() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let workspaces_dir = repo_path.join(".bacchus/workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let workspace_path = workspaces_dir.join("test-task-2");

        // Create workspace
        Command::new("jj")
            .args(["workspace", "add", "--name", "test-task-2"])
            .arg(&workspace_path)
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Make changes in workspace (jj auto-snapshots)
        fs::write(workspace_path.join("new_file.txt"), "new content").unwrap();

        // Describe the change
        let output = Command::new("jj")
            .args([
                "-R",
                workspace_path.to_str().unwrap(),
                "describe",
                "-m",
                "Add new file",
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "Describe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Check status shows the file
        let output = Command::new("jj")
            .args(["-R", workspace_path.to_str().unwrap(), "status"])
            .output()
            .unwrap();

        assert!(output.status.success());
    }

    #[test]
    fn test_workspace_removal() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let workspaces_dir = repo_path.join(".bacchus/workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let workspace_path = workspaces_dir.join("test-task-3");

        // Create workspace
        Command::new("jj")
            .args(["workspace", "add", "--name", "test-task-3"])
            .arg(&workspace_path)
            .current_dir(&repo_path)
            .output()
            .unwrap();

        assert!(workspace_path.exists());

        // Forget workspace (unlinks but may not delete dir)
        let output = Command::new("jj")
            .args(["workspace", "forget", "test-task-3"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "Workspace forget failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Clean up directory
        if workspace_path.exists() {
            fs::remove_dir_all(&workspace_path).unwrap();
        }

        // Verify workspace is gone from list
        let output = Command::new("jj")
            .args(["workspace", "list"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("test-task-3"));
    }

    #[test]
    fn test_conflict_detection() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let workspaces_dir = repo_path.join(".bacchus/workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let workspace_path = workspaces_dir.join("test-conflict");

        // Create workspace
        Command::new("jj")
            .args(["workspace", "add", "--name", "test-conflict"])
            .arg(&workspace_path)
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Modify test.txt in workspace
        fs::write(workspace_path.join("test.txt"), "workspace change").unwrap();
        Command::new("jj")
            .args([
                "-R",
                workspace_path.to_str().unwrap(),
                "describe",
                "-m",
                "Workspace change",
            ])
            .output()
            .unwrap();

        // Modify test.txt in main (different content)
        fs::write(repo_path.join("test.txt"), "main change").unwrap();
        Command::new("jj")
            .args(["describe", "-m", "Main change"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Rebase workspace onto main - this may produce conflicts
        let output = Command::new("jj")
            .args([
                "-R",
                workspace_path.to_str().unwrap(),
                "rebase",
                "-d",
                "main",
            ])
            .output()
            .unwrap();

        // Check for conflicts (jj allows conflicts, we check status)
        let status_output = Command::new("jj")
            .args(["-R", workspace_path.to_str().unwrap(), "status"])
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&status_output.stdout);
        // jj shows conflicts in status output
        // The test passes if we can detect the situation (conflict or successful rebase)
        assert!(output.status.success() || stdout.contains("conflict"));
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
        let output = Command::new(bacchus_bin())
            .args(["status"])
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success(), "DB init failed: {:?}", output);
        db_path
    }

    #[test]
    fn test_init_command_creates_bacchus_scaffold() {
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path();

        let output = Command::new(bacchus_bin())
            .args(["init", "--skip-jj"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(repo_path.join(".bacchus/tasks.yaml").exists());
        assert!(repo_path.join(".bacchus/bacchus.db").exists());
    }

    #[test]
    fn test_init_command_creates_epic_when_requested() {
        let temp = TempDir::new().unwrap();
        let repo_path = temp.path();

        let init_output = Command::new(bacchus_bin())
            .args([
                "init",
                "--skip-jj",
                "--epic-id",
                "INIT-SMOKE",
                "--epic-title",
                "Init Smoke",
            ])
            .current_dir(repo_path)
            .output()
            .unwrap();
        assert!(
            init_output.status.success(),
            "init failed: {}",
            String::from_utf8_lossy(&init_output.stderr)
        );

        let show_output = Command::new(bacchus_bin())
            .args(["epic", "list"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        assert!(
            show_output.status.success(),
            "epic list failed: {}",
            String::from_utf8_lossy(&show_output.stderr)
        );
        let stdout = String::from_utf8_lossy(&show_output.stdout);
        assert!(stdout.contains("\"id\": \"INIT-SMOKE\""));
    }

    #[test]
    fn test_claim_operations() {
        let temp = TempDir::new().unwrap();
        let db_path = init_test_db(&temp);

        // Verify status shows empty claims
        let output = Command::new(bacchus_bin())
            .args(["status"])
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

        let output = Command::new(bacchus_bin())
            .args(["list"])
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

        let output = Command::new(bacchus_bin())
            .args(["stale", "--minutes", "1"])
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("stale_claims"));
    }

    #[test]
    fn test_claim_workspace_failure_rolls_back_to_open() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: ROLLBACK-001
    title: "Rollback claim test"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/app.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "ROLLBACK"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let bad_workspaces = repo_path.join("bad-workspaces");
        fs::write(&bad_workspaces, "not-a-directory").unwrap();

        let claim_output = Command::new(bacchus_bin())
            .args(["claim", "ROLLBACK-001", "agent-1"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_WORKSPACES", &bad_workspaces)
            .output()
            .unwrap();
        assert!(!claim_output.status.success());

        let show_output = Command::new(bacchus_bin())
            .args(["task", "show", "ROLLBACK-001"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(show_output.status.success());
        let show_stdout = String::from_utf8_lossy(&show_output.stdout);
        assert!(
            show_stdout.contains("\"status\": \"open\""),
            "Expected task to remain open after failed claim, got: {}",
            show_stdout
        );
    }

    #[test]
    fn test_next_workspace_failure_rolls_back_to_open() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: ROLLBACK-002
    title: "Rollback next test"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/lib.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "ROLLBACK"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let bad_workspaces = repo_path.join("bad-workspaces-next");
        fs::write(&bad_workspaces, "not-a-directory").unwrap();

        let next_output = Command::new(bacchus_bin())
            .args(["next", "agent-1"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_WORKSPACES", &bad_workspaces)
            .output()
            .unwrap();
        assert!(!next_output.status.success());

        let show_output = Command::new(bacchus_bin())
            .args(["task", "show", "ROLLBACK-002"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(show_output.status.success());
        let show_stdout = String::from_utf8_lossy(&show_output.stdout);
        assert!(
            show_stdout.contains("\"status\": \"open\""),
            "Expected task to remain open after failed next, got: {}",
            show_stdout
        );
    }

    #[test]
    fn test_orchestrator_session_ignores_stale_claim_capacity() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: STALE-001
    title: "Stale claimed task"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/a.rs"]
      creates: []
  - id: STALE-002
    title: "Ready task"
    priority: 2
    status: open
    depends_on: []
    footprint:
      modifies: ["src/b.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "STALE"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let stale_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
            - (16 * 60 * 1000);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE tasks
             SET status = 'in_progress',
                 claimed_by = 'agent-stale',
                 claimed_at = ?1,
                 claimed_heartbeat_at = ?1
             WHERE id = 'STALE-001'",
            [stale_ts],
        )
        .unwrap();

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(start_output.status.success());

        let check_output = Command::new(bacchus_bin())
            .args(["session", "check"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(check_output.status.success());

        let check_stdout = String::from_utf8_lossy(&check_output.stdout);
        assert!(
            check_stdout.contains("Ready to spawn 1 agent(s)"),
            "Expected stale claim to not consume capacity, got: {}",
            check_stdout
        );
    }

    #[test]
    fn test_orchestrator_session_auto_spawns_worker_when_configured() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join("test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: SWARM-001
    title: "Spawn worker task"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/swarm.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "SWARM"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_WORKER_CMD", "echo worker")
            .output()
            .unwrap();
        assert!(start_output.status.success());

        let check_output = Command::new(bacchus_bin())
            .args(["session", "check"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_WORKER_CMD", "echo worker")
            .output()
            .unwrap();
        assert!(check_output.status.success());
        let check_stdout = String::from_utf8_lossy(&check_output.stdout);
        assert!(
            check_stdout.contains("Auto-spawn attempted 1 task(s): launched 1"),
            "Expected auto-spawn launch note, got: {}",
            check_stdout
        );

        std::thread::sleep(std::time::Duration::from_millis(200));

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let workers_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_workers WHERE run_id LIKE 'orchestrator-%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(workers_count >= 1);

        let task_status: String = conn
            .query_row("SELECT status FROM tasks WHERE id = 'SWARM-001'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(task_status, "in_progress");
    }

    #[test]
    fn test_session_spawn_workers_command_dry_run_and_launch() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join("test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: SWARM-002
    title: "Spawn worker command task"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/swarm2.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "SWARM"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_WORKER_CMD", "echo worker")
            .output()
            .unwrap();
        assert!(start_output.status.success());

        let dry_run_output = Command::new(bacchus_bin())
            .args(["session", "spawn-workers", "--count", "1", "--dry-run"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(dry_run_output.status.success());
        let dry_run_json: Value = serde_json::from_slice(&dry_run_output.stdout).unwrap();
        assert_eq!(dry_run_json["dry_run"], true);
        let has_candidate = dry_run_json["candidate_tasks"]
            .as_array()
            .is_some_and(|arr| arr.iter().any(|t| t.as_str() == Some("SWARM-002")));
        assert!(has_candidate);

        let spawn_output = Command::new(bacchus_bin())
            .args(["session", "spawn-workers", "--count", "1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_WORKER_CMD", "echo worker")
            .output()
            .unwrap();
        assert!(spawn_output.status.success());
        let spawn_json: Value = serde_json::from_slice(&spawn_output.stdout).unwrap();
        assert!(
            spawn_json["spawn_summary"]["launched"]
                .as_i64()
                .unwrap_or(0)
                >= 1,
            "Expected at least one launch: {}",
            String::from_utf8_lossy(&spawn_output.stdout)
        );

        std::thread::sleep(std::time::Duration::from_millis(200));
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let workers_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_workers WHERE run_id LIKE 'orchestrator-%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(workers_count >= 1);
    }

    #[test]
    fn test_orchestrator_check_recovers_stale_running_worker() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join("test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: STALEW-001
    title: "Recover stale worker"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/stalew.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "SWARM"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(start_output.status.success());

        let session_status = Command::new(bacchus_bin())
            .args(["session", "status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(session_status.status.success());
        let session_json: Value = serde_json::from_slice(&session_status.stdout).unwrap();
        let run_id = session_json["session"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();

        let stale_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
            - (20 * 60 * 1000);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE tasks
             SET status = 'in_progress',
                 claimed_by = 'agent-stalew',
                 claimed_at = ?1,
                 claimed_heartbeat_at = ?1
             WHERE id = 'STALEW-001'",
            [stale_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_workers
             (run_id, task_id, agent_id, scope_id, command, status, attempt, pid, started_at, updated_at)
             VALUES (?1, 'STALEW-001', 'agent-stalew', 'scope-stale', 'echo worker', 'running', 1, 42424, ?2, ?2)",
            rusqlite::params![run_id, stale_ts],
        )
        .unwrap();

        let check_output = Command::new(bacchus_bin())
            .args(["session", "check"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(check_output.status.success());
        let check_stdout = String::from_utf8_lossy(&check_output.stdout);
        assert!(
            check_stdout.contains("Recovered stale workers: 1"),
            "Expected stale-worker recovery note, got: {}",
            check_stdout
        );

        let worker_status: String = conn
            .query_row(
                "SELECT status FROM agent_workers WHERE run_id = ?1 AND task_id = 'STALEW-001'",
                [run_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(worker_status, "failed");

        let task_row: (String, Option<String>) = conn
            .query_row(
                "SELECT status, claimed_by FROM tasks WHERE id = 'STALEW-001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(task_row.0, "open");
        assert!(task_row.1.is_none());

        let recovery_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM orchestration_events WHERE event_type = 'worker_stale_recovered' AND run_id = ?1",
                [run_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(recovery_events >= 1);
    }

    #[test]
    fn test_orchestrator_check_recovers_worker_on_runtime_timeout() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join("test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: TIMEOUTW-001
    title: "Recover timed-out worker"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/timeoutw.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "SWARM"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(start_output.status.success());

        let session_status = Command::new(bacchus_bin())
            .args(["session", "status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(session_status.status.success());
        let session_json: Value = serde_json::from_slice(&session_status.stdout).unwrap();
        let run_id = session_json["session"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();

        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let old_started = now_ts - 10_000;
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE tasks
             SET status = 'in_progress',
                 claimed_by = 'agent-timeoutw',
                 claimed_at = ?1,
                 claimed_heartbeat_at = ?1
             WHERE id = 'TIMEOUTW-001'",
            [now_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_workers
             (run_id, task_id, agent_id, scope_id, command, status, attempt, pid, started_at, updated_at)
             VALUES (?1, 'TIMEOUTW-001', 'agent-timeoutw', 'scope-timeout', 'echo worker', 'running', 1, 43434, ?2, ?2)",
            rusqlite::params![run_id, old_started],
        )
        .unwrap();

        let check_output = Command::new(bacchus_bin())
            .args(["session", "check"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_WORKER_MAX_RUNTIME_MS", "1000")
            .output()
            .unwrap();
        assert!(check_output.status.success());
        let check_stdout = String::from_utf8_lossy(&check_output.stdout);
        assert!(
            check_stdout.contains("Recovered stale workers: 1"),
            "Expected timeout recovery note, got: {}",
            check_stdout
        );

        let worker_status: String = conn
            .query_row(
                "SELECT status FROM agent_workers WHERE run_id = ?1 AND task_id = 'TIMEOUTW-001'",
                [run_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(worker_status, "failed");

        let task_status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE id = 'TIMEOUTW-001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(task_status, "open");
    }

    #[test]
    fn test_orchestrator_stop_reopens_tasks_for_active_workers() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join("test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: STOPW-001
    title: "Stop session worker reopen"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/stopw.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "SWARM"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(start_output.status.success());

        let session_status = Command::new(bacchus_bin())
            .args(["session", "status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(session_status.status.success());
        let session_json: Value = serde_json::from_slice(&session_status.stdout).unwrap();
        let run_id = session_json["session"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();

        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE tasks
             SET status = 'in_progress',
                 claimed_by = 'agent-stopw',
                 claimed_at = ?1,
                 claimed_heartbeat_at = ?1
             WHERE id = 'STOPW-001'",
            [now_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_workers
             (run_id, task_id, agent_id, scope_id, command, status, attempt, pid, started_at, updated_at)
             VALUES (?1, 'STOPW-001', 'agent-stopw', 'scope-stop', 'echo worker', 'running', 1, 45454, ?2, ?2)",
            rusqlite::params![run_id, now_ts],
        )
        .unwrap();

        let stop_output = Command::new(bacchus_bin())
            .args(["session", "stop"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(stop_output.status.success());

        let worker_status: String = conn
            .query_row(
                "SELECT status FROM agent_workers WHERE run_id = ?1 AND task_id = 'STOPW-001'",
                [run_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(worker_status, "failed");

        let task_row: (String, Option<String>) = conn
            .query_row(
                "SELECT status, claimed_by FROM tasks WHERE id = 'STOPW-001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(task_row.0, "open");
        assert!(task_row.1.is_none());
    }

    #[test]
    fn test_orchestrator_check_recovers_worker_when_pid_is_dead() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join("test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: PIDW-001
    title: "Recover dead pid worker"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/pidw.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "SWARM"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(start_output.status.success());

        let session_status = Command::new(bacchus_bin())
            .args(["session", "status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(session_status.status.success());
        let session_json: Value = serde_json::from_slice(&session_status.stdout).unwrap();
        let run_id = session_json["session"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();

        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE tasks
             SET status = 'in_progress',
                 claimed_by = 'agent-pidw',
                 claimed_at = ?1,
                 claimed_heartbeat_at = ?1
             WHERE id = 'PIDW-001'",
            [now_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_workers
             (run_id, task_id, agent_id, scope_id, command, status, attempt, pid, started_at, updated_at)
             VALUES (?1, 'PIDW-001', 'agent-pidw', 'scope-pid', 'echo worker', 'running', 1, 999999, ?2, ?2)",
            rusqlite::params![run_id, now_ts],
        )
        .unwrap();

        let check_output = Command::new(bacchus_bin())
            .args(["session", "check"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(check_output.status.success());
        let check_stdout = String::from_utf8_lossy(&check_output.stdout);
        assert!(
            check_stdout.contains("Recovered stale workers: 1"),
            "Expected dead-pid recovery note, got: {}",
            check_stdout
        );

        let worker_status: String = conn
            .query_row(
                "SELECT status FROM agent_workers WHERE run_id = ?1 AND task_id = 'PIDW-001'",
                [run_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(worker_status, "failed");
    }

    #[test]
    fn test_orchestrator_check_reopens_failed_worker_owned_task() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join("test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: FWR-001
    title: "Reopen failed worker task"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["src/fwr.rs"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "SWARM"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(start_output.status.success());

        let session_status = Command::new(bacchus_bin())
            .args(["session", "status"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(session_status.status.success());
        let session_json: Value = serde_json::from_slice(&session_status.stdout).unwrap();
        let run_id = session_json["session"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();

        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE tasks
             SET status = 'in_progress',
                 claimed_by = 'agent-fwr',
                 claimed_at = ?1,
                 claimed_heartbeat_at = ?1
             WHERE id = 'FWR-001'",
            [now_ts],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_workers
             (run_id, task_id, agent_id, scope_id, command, status, attempt, started_at, updated_at, error)
             VALUES (?1, 'FWR-001', 'agent-fwr', 'scope-fwr', 'echo worker', 'failed', 1, ?2, ?2, 'boom')",
            rusqlite::params![run_id, now_ts],
        )
        .unwrap();

        let check_output = Command::new(bacchus_bin())
            .args(["session", "check"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(check_output.status.success());
        let check_stdout = String::from_utf8_lossy(&check_output.stdout);
        assert!(
            check_stdout.contains("Reopened tasks from failed workers: 1"),
            "Expected failed-worker reopen note, got: {}",
            check_stdout
        );

        let task_row: (String, Option<String>) = conn
            .query_row(
                "SELECT status, claimed_by FROM tasks WHERE id = 'FWR-001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(task_row.0, "open");
        assert!(task_row.1.is_none());
    }

    #[test]
    fn test_status_separates_stale_claims_from_active_count() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let stale = now - (16 * 60 * 1000);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at)
             VALUES ('STATUS-EPIC', 'Status Epic', 'open', 'test', ?1, ?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, created_at, updated_at)
             VALUES ('STATUS-001', 'STATUS-EPIC', 'Stale claim', 1, 'in_progress', 'generic', 'generic', 'agent-stale', ?1, ?1, ?2, ?2)",
            rusqlite::params![stale, now],
        )
        .unwrap();

        let status_output = Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(status_output.status.success());
        let status_json: Value = serde_json::from_slice(&status_output.stdout).unwrap();

        assert_eq!(status_json["claims"]["count"], 0);
        assert_eq!(status_json["claims"]["stale_count"], 1);
    }

    #[test]
    fn test_orchestrator_lease_blocks_second_session_start() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let start_one = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(start_one.status.success());

        let start_two = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(!start_two.status.success());
        let stderr = String::from_utf8_lossy(&start_two.stderr);
        assert!(
            stderr.contains("leader lease"),
            "Expected lease contention error, got: {}",
            stderr
        );
    }

    #[test]
    fn test_scoped_agent_sessions_do_not_clobber_each_other() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at)
             VALUES ('SCOPE-EPIC', 'Scope Epic', 'open', 'test', ?1, ?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, created_at, updated_at)
             VALUES ('SCOPE-001', 'SCOPE-EPIC', 'Scoped A', 1, 'in_progress', 'generic', 'generic', 'agent-a', ?1, ?1, ?2, ?2)",
            rusqlite::params![now, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, created_at, updated_at)
             VALUES ('SCOPE-002', 'SCOPE-EPIC', 'Scoped B', 2, 'in_progress', 'generic', 'generic', 'agent-b', ?1, ?1, ?2, ?2)",
            rusqlite::params![now, now],
        )
        .unwrap();

        let start_a = Command::new(bacchus_bin())
            .args([
                "session",
                "start",
                "agent",
                "--task-id",
                "SCOPE-001",
                "--agent-id",
                "agent-a",
            ])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "scope-a")
            .env("BACCHUS_AGENT_HEARTBEAT_INTERVAL_MS", "100")
            .output()
            .unwrap();
        assert!(start_a.status.success());

        let start_b = Command::new(bacchus_bin())
            .args([
                "session",
                "start",
                "agent",
                "--task-id",
                "SCOPE-002",
                "--agent-id",
                "agent-b",
            ])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "scope-b")
            .env("BACCHUS_AGENT_HEARTBEAT_INTERVAL_MS", "100")
            .output()
            .unwrap();
        assert!(start_b.status.success());

        let status_a = Command::new(bacchus_bin())
            .args(["session", "status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "scope-a")
            .output()
            .unwrap();
        let status_a_json: Value = serde_json::from_slice(&status_a.stdout).unwrap();
        assert_eq!(status_a_json["session"]["task_id"], "SCOPE-001");

        let status_b = Command::new(bacchus_bin())
            .args(["session", "status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "scope-b")
            .output()
            .unwrap();
        let status_b_json: Value = serde_json::from_slice(&status_b.stdout).unwrap();
        assert_eq!(status_b_json["session"]["task_id"], "SCOPE-002");

        Command::new(bacchus_bin())
            .args(["session", "stop"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "scope-a")
            .output()
            .unwrap();
        Command::new(bacchus_bin())
            .args(["session", "stop"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "scope-b")
            .output()
            .unwrap();
    }

    #[test]
    fn test_orchestrator_lease_loop_renews_without_check_calls() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let start_primary = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "orch-a")
            .env("BACCHUS_ORCHESTRATOR_LEASE_TTL_MS", "500")
            .env("BACCHUS_ORCHESTRATOR_LEASE_INTERVAL_MS", "100")
            .output()
            .unwrap();
        assert!(start_primary.status.success());

        std::thread::sleep(std::time::Duration::from_millis(1200));

        let start_secondary = Command::new(bacchus_bin())
            .args(["session", "start", "orchestrator", "--max-concurrent", "1"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "orch-b")
            .env("BACCHUS_ORCHESTRATOR_LEASE_TTL_MS", "500")
            .env("BACCHUS_ORCHESTRATOR_LEASE_INTERVAL_MS", "100")
            .output()
            .unwrap();
        assert!(!start_secondary.status.success());

        Command::new(bacchus_bin())
            .args(["session", "stop"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "orch-a")
            .output()
            .unwrap();
    }

    #[test]
    fn test_default_scope_start_rejects_concurrent_agent_session() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at)
             VALUES ('DEF-EPIC', 'Default Scope Epic', 'open', 'test', ?1, ?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, created_at, updated_at)
             VALUES ('DEF-001', 'DEF-EPIC', 'Default A', 1, 'in_progress', 'generic', 'generic', 'agent-a', ?1, ?1, ?2, ?2)",
            rusqlite::params![now, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, created_at, updated_at)
             VALUES ('DEF-002', 'DEF-EPIC', 'Default B', 2, 'in_progress', 'generic', 'generic', 'agent-b', ?1, ?1, ?2, ?2)",
            rusqlite::params![now, now],
        )
        .unwrap();

        let first = Command::new(bacchus_bin())
            .args([
                "session",
                "start",
                "agent",
                "--task-id",
                "DEF-001",
                "--agent-id",
                "agent-a",
            ])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(first.status.success());

        let second = Command::new(bacchus_bin())
            .args([
                "session",
                "start",
                "agent",
                "--task-id",
                "DEF-002",
                "--agent-id",
                "agent-b",
            ])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(!second.status.success());
        let stderr = String::from_utf8_lossy(&second.stderr);
        assert!(
            stderr.contains("BACCHUS_SESSION_ID"),
            "Expected default-scope collision guidance, got: {}",
            stderr
        );
    }

    #[test]
    fn test_session_prune_removes_stale_scope_file() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus/sessions")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let stale_started = "2000-01-01T00:00:00Z";
        let stale_session = serde_json::json!({
            "mode": "agent",
            "task_id": "STALE-SCOPE",
            "started_at": stale_started
        });
        fs::write(
            repo_path.join(".bacchus/sessions/stale-scope.json"),
            serde_json::to_string_pretty(&stale_session).unwrap(),
        )
        .unwrap();

        let prune_output = Command::new(bacchus_bin())
            .args(["session", "prune", "--minutes", "5"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_SESSION_ID", "active-scope")
            .output()
            .unwrap();
        assert!(prune_output.status.success());
        let prune_json: Value = serde_json::from_slice(&prune_output.stdout).unwrap();
        assert!(prune_json["removed_scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("stale-scope")));
    }

    #[test]
    fn test_agent_session_background_heartbeat_updates_claim() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let old_heartbeat = now - (30 * 60 * 1000);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at)
             VALUES ('HB-EPIC', 'Heartbeat Epic', 'open', 'test', ?1, ?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, created_at, updated_at)
             VALUES ('HB-001', 'HB-EPIC', 'Heartbeat task', 1, 'in_progress', 'generic', 'generic', 'agent-hb', ?1, ?1, ?2, ?2)",
            rusqlite::params![old_heartbeat, now],
        )
        .unwrap();

        let start_output = Command::new(bacchus_bin())
            .args(["session", "start", "agent", "--task-id", "HB-001"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .env("BACCHUS_AGENT_HEARTBEAT_INTERVAL_MS", "100")
            .output()
            .unwrap();
        assert!(start_output.status.success());

        std::thread::sleep(std::time::Duration::from_millis(450));

        let latest: i64 = rusqlite::Connection::open(&db_path)
            .unwrap()
            .query_row(
                "SELECT COALESCE(claimed_heartbeat_at, 0) FROM tasks WHERE id = 'HB-001'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            latest > old_heartbeat,
            "Expected heartbeat loop to refresh timestamp, old={}, new={}",
            old_heartbeat,
            latest
        );

        Command::new(bacchus_bin())
            .args(["session", "stop"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
    }

    #[test]
    fn test_process_releases_events_include_run_id() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let stale_release = now - (11 * 60 * 1000);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at)
             VALUES ('EVT-EPIC', 'Event Epic', 'open', 'test', ?1, ?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, release_started_at, created_at, updated_at)
             VALUES ('EVT-001', 'EVT-EPIC', 'Stale release task', 1, 'releasing', 'generic', 'generic', 'agent-evt', ?1, ?1, ?2, ?3, ?3)",
            rusqlite::params![now, stale_release, now],
        )
        .unwrap();

        let process_output = Command::new(bacchus_bin())
            .args(["process-releases"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(process_output.status.success());

        let events_output = Command::new(bacchus_bin())
            .args(["events", "--limit", "10"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(events_output.status.success());
        let events: Value = serde_json::from_slice(&events_output.stdout).unwrap();
        let entries = events.as_array().unwrap();
        let has_run_id = entries.iter().any(|e| {
            e.get("event_type")
                .and_then(|v| v.as_str())
                .map(|t| t == "release_reset_timeout")
                .unwrap_or(false)
                && e.get("run_id")
                    .and_then(|v| v.as_str())
                    .map(|id| id.starts_with("manual-orchestrator-"))
                    .unwrap_or(false)
        });

        assert!(has_run_id, "Expected run_id on release_reset_timeout event");
    }

    #[test]
    fn test_list_splits_active_and_stale_claims() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");
        let repo_path = temp.path();

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();

        Command::new(bacchus_bin())
            .args(["status"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let stale = now - (20 * 60 * 1000);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO epics (id, title, status, created_by, created_at, updated_at)
             VALUES ('LIST-EPIC', 'List Epic', 'open', 'test', ?1, ?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, created_at, updated_at)
             VALUES ('LIST-ACTIVE', 'LIST-EPIC', 'Active claim', 1, 'in_progress', 'generic', 'generic', 'agent-active', ?1, ?1, ?2, ?2)",
            rusqlite::params![now, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, epic_id, title, priority, status, task_type, archetype, claimed_by, claimed_at, claimed_heartbeat_at, created_at, updated_at)
             VALUES ('LIST-STALE', 'LIST-EPIC', 'Stale claim', 2, 'in_progress', 'generic', 'generic', 'agent-stale', ?1, ?1, ?2, ?2)",
            rusqlite::params![stale, now],
        )
        .unwrap();

        let list_output = Command::new(bacchus_bin())
            .args(["list"])
            .current_dir(repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(list_output.status.success());
        let list_json: Value = serde_json::from_slice(&list_output.stdout).unwrap();

        assert_eq!(list_json["active_total"], 1);
        assert_eq!(list_json["stale_total"], 1);
        assert_eq!(list_json["claims"].as_array().unwrap().len(), 1);
        assert_eq!(list_json["stale_claims"].as_array().unwrap().len(), 1);
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
        let output = Command::new(bacchus_bin())
            .args(["index"])
            .arg(&test_file)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success(), "Index failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("files_indexed"));

        // Search for the function
        let output = Command::new(bacchus_bin())
            .args(["symbols", "--pattern", "greet"])
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("greet") || stdout.contains("symbols"));

        // Search for the class
        let output = Command::new(bacchus_bin())
            .args(["symbols", "--pattern", "Greeter", "--kind", "class"])
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
        let output = Command::new(bacchus_bin())
            .args(["--help"])
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Workspace-based coordination"));
    }

    #[test]
    fn test_workflow_doc() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        let output = Command::new(bacchus_bin())
            .args(["workflow"])
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
        let output = Command::new(bacchus_bin())
            .args(["--version"])
            .output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("bacchus"));
    }
}

// ============================================================================
// Full Workflow Tests
// ============================================================================

mod workflow_tests {
    use super::*;

    #[test]
    fn test_next_without_ready_tasks() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (temp, repo_path) = init_test_repo();
        let db_path = temp.path().join("test.db");

        // Run next - should report no ready tasks (no tasks.yaml exists)
        let output = Command::new(bacchus_bin())
            .args(["next", "test-agent"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Should indicate no ready tasks or tasks file not found
        assert!(
            stdout.contains("No ready tasks")
                || stdout.contains("success\": false")
                || stdout.contains("Tasks file not found"),
            "Unexpected output: {}",
            stdout
        );
    }

    #[test]
    fn test_release_without_claim() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // Init DB first
        Command::new(bacchus_bin())
            .args(["status"])
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        // Try to release non-existent claim
        let output = Command::new(bacchus_bin())
            .args(["release", "nonexistent-task", "--status", "done"])
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("not found") || stdout.contains("success\": false"),
            "Expected 'not found', got: {}",
            stdout
        );
    }

    #[test]
    fn test_abort_without_needs_resolution() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // Init DB first
        Command::new(bacchus_bin())
            .args(["status"])
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        // Try to abort when task doesn't exist or isn't in needs_resolution
        let output = Command::new(bacchus_bin())
            .args(["abort", "test-task"])
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        // Should fail or report task not in needs_resolution
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("not found")
                || stdout.contains("not in needs_resolution")
                || stdout.contains("success\": false")
                || !output.status.success(),
            "Expected error, got: stdout={}, stderr={}",
            stdout,
            stderr
        );
    }

    #[test]
    fn test_process_releases_closes_ready_task() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join(".bacchus/test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: REL-001
    title: "Release task"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["test.txt"]
      creates: []
"#,
        )
        .unwrap();

        let import_output = Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "REL"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(import_output.status.success());

        let claim_output = Command::new(bacchus_bin())
            .args(["claim", "REL-001", "agent-1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(claim_output.status.success());

        let workspace_path = repo_path.join(".bacchus/workspaces/REL-001");
        fs::write(workspace_path.join("test.txt"), "release change").unwrap();

        let describe_output = Command::new("jj")
            .args([
                "-R",
                workspace_path.to_str().unwrap(),
                "describe",
                "-m",
                "Release task change",
            ])
            .output()
            .unwrap();
        assert!(
            describe_output.status.success(),
            "describe failed: {}",
            String::from_utf8_lossy(&describe_output.stderr)
        );

        let ready_output = Command::new(bacchus_bin())
            .args(["release", "REL-001", "--status", "done"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(ready_output.status.success());

        let process_output = Command::new(bacchus_bin())
            .args(["process-releases"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(process_output.status.success());
        let process_stdout = String::from_utf8_lossy(&process_output.stdout);
        assert!(
            process_stdout.contains("\"merged\": 1"),
            "Expected merged count, got: {}",
            process_stdout
        );

        let show_output = Command::new(bacchus_bin())
            .args(["task", "show", "REL-001"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(show_output.status.success());
        let show_stdout = String::from_utf8_lossy(&show_output.stdout);
        assert!(
            show_stdout.contains("\"status\": \"closed\""),
            "Expected closed status, got: {}",
            show_stdout
        );
    }

    #[test]
    fn test_force_claim_rejects_ready_for_release_state() {
        if !jj_installed() {
            eprintln!("Skipping test: jj not installed");
            return;
        }

        let (_temp, repo_path) = init_test_repo();
        let db_path = repo_path.join(".bacchus/test.db");

        fs::create_dir_all(repo_path.join(".bacchus")).unwrap();
        fs::write(
            repo_path.join(".bacchus/tasks.yaml"),
            r#"
version: 1
tasks:
  - id: FORCE-001
    title: "Force guard task"
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies: ["test.txt"]
      creates: []
"#,
        )
        .unwrap();

        Command::new(bacchus_bin())
            .args(["task", "import", "--epic-id", "FORCE"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        Command::new(bacchus_bin())
            .args(["claim", "FORCE-001", "agent-1"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let workspace_path = repo_path.join(".bacchus/workspaces/FORCE-001");
        fs::write(workspace_path.join("test.txt"), "force guard change").unwrap();
        Command::new("jj")
            .args([
                "-R",
                workspace_path.to_str().unwrap(),
                "describe",
                "-m",
                "Force guard change",
            ])
            .output()
            .unwrap();

        Command::new(bacchus_bin())
            .args(["release", "FORCE-001", "--status", "done"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        let force_output = Command::new(bacchus_bin())
            .args(["claim", "FORCE-001", "agent-2", "--force"])
            .current_dir(&repo_path)
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();
        assert!(force_output.status.success());
        let force_stdout = String::from_utf8_lossy(&force_output.stdout);
        assert!(
            force_stdout.contains("cannot be force-claimed"),
            "Expected force-claim guard message, got: {}",
            force_stdout
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
        let output = Command::new(bacchus_bin())
            .args(["invalid-command"])
            .output()
            .unwrap();

        assert!(!output.status.success());
    }

    #[test]
    fn test_missing_arguments() {
        let output = Command::new(bacchus_bin()).args(["next"]).output().unwrap();

        // Should fail due to missing agent_id
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("agent_id") || stderr.contains("required"));
    }

    #[test]
    fn test_invalid_release_status() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.db");

        // Try invalid status — clap rejects before release_task is called
        let output = Command::new(bacchus_bin())
            .args(["release", "test", "--status", "invalid"])
            .env("BACCHUS_DB_PATH", &db_path)
            .output()
            .unwrap();

        assert!(
            !output.status.success(),
            "Expected non-zero exit for invalid status"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("invalid value 'invalid'"),
            "Expected clap validation error, got: {}",
            stderr
        );
    }
}
