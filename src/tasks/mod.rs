//! Task management module
//!
//! Uses SQLite-based tasks (tasks table) for hierarchical orchestration.
//! YAML tasks (.bacchus/tasks.yaml) are read-only and used for import.
//!
//! ## Task Storage
//! - **SQLite tasks**: Primary format in `tasks` table (must belong to an epic)
//! - **YAML tasks**: Read-only for `bacchus task import` migration
//!
//! ## Workflow
//! 1. Initialize tasks via `task init` (creates YAML template)
//! 2. Import to SQLite via `task import --epic-id EPIC`
//! 3. All runtime operations use SQLite

mod crud;
mod lease;
pub(crate) mod queries;
mod readiness;
pub mod types;
mod validation;
mod yaml;

// Re-export all public items to maintain backward compatibility
// (every `pub fn` and `pub struct/enum` accessible as `crate::tasks::*`)

pub use types::{
    CreateSqliteTaskInput, SqliteTask, SqliteTaskStatus, SqliteTaskType, Task, TaskFootprint,
    TaskValidation, TasksError,
};

#[cfg(test)]
pub use types::{ResolvedFootprint, TasksFile};

pub use crud::{
    complete_task_release, create_sqlite_task, get_sqlite_task, get_tasks_ready_for_release,
    heartbeat_sqlite_task, list_sqlite_tasks, mark_task_needs_resolution,
    mark_task_ready_for_release, reset_sqlite_task, reset_task_from_resolution,
    reset_task_release_to_ready, set_task_release_commit, start_task_release,
};

pub use readiness::{claim_next_sqlite_task, claim_sqlite_task, get_ready_sqlite_tasks};

#[cfg(test)]
pub use readiness::release_sqlite_task;

pub use validation::validate_tasks;

#[cfg(test)]
pub use validation::{footprints_overlap, normalize_footprint};

pub use yaml::{generate_template, import_yaml_tasks, tasks_file_path};

// Lease management
pub use lease::{
    get_orchestrator_lease, release_orchestrator_lease, try_acquire_orchestrator_lease,
    CLAIM_HEARTBEAT_TIMEOUT_MS, ORCHESTRATOR_LEASE_TTL_MS,
};

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::with_db;

    #[test]
    fn test_task_footprint_default() {
        let footprint = TaskFootprint::default();
        assert!(footprint.modifies.is_empty());
        assert!(footprint.creates.is_empty());
    }

    #[test]
    fn test_footprints_overlap_symbols() {
        let mut a = ResolvedFootprint::default();
        a.symbols.insert("src/auth.rs::login".to_string());

        let mut b = ResolvedFootprint::default();
        b.symbols.insert("src/auth.rs::login".to_string());

        assert!(footprints_overlap(&a, &b));
    }

    #[test]
    fn test_footprints_no_overlap() {
        let mut a = ResolvedFootprint::default();
        a.symbols.insert("src/auth.rs::login".to_string());

        let mut b = ResolvedFootprint::default();
        b.symbols.insert("src/user.rs::create".to_string());

        assert!(!footprints_overlap(&a, &b));
    }

    #[test]
    fn test_footprints_overlap_creates() {
        let mut a = ResolvedFootprint::default();
        a.creates.insert("src/new_file.rs".to_string());

        let mut b = ResolvedFootprint::default();
        b.creates.insert("src/new_file.rs".to_string());

        assert!(footprints_overlap(&a, &b));
    }

    #[test]
    fn test_parse_task_yaml() {
        let yaml = r#"
version: 1
tasks:
  - id: TEST-001
    title: Test task
    priority: 1
    status: open
    depends_on: []
    footprint:
      modifies:
        - "src/test.rs::*"
      creates:
        - "src/new.rs"
"#;
        let tasks_file: TasksFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(tasks_file.version, 1);
        assert_eq!(tasks_file.tasks.len(), 1);
        assert_eq!(tasks_file.tasks[0].id, "TEST-001");
        assert_eq!(tasks_file.tasks[0].footprint.modifies.len(), 1);
        assert_eq!(tasks_file.tasks[0].footprint.creates.len(), 1);
    }

    #[test]
    fn test_default_values() {
        let yaml = r#"
version: 1
tasks:
  - id: MINIMAL
    title: Minimal task
"#;
        let tasks_file: TasksFile = serde_yaml::from_str(yaml).unwrap();
        let task = &tasks_file.tasks[0];
        assert_eq!(task.priority, 5); // default
        assert_eq!(task.status, "open"); // default
        assert!(task.depends_on.is_empty()); // default
        assert!(task.footprint.modifies.is_empty()); // default
    }

    #[test]
    fn test_footprints_wildcard_overlap() {
        // file::* should match file::specific_symbol
        let mut a = ResolvedFootprint::default();
        a.symbols.insert("src/auth.rs::*".to_string());

        let mut b = ResolvedFootprint::default();
        b.symbols.insert("src/auth.rs::login".to_string());

        assert!(footprints_overlap(&a, &b));
    }

    #[test]
    fn test_footprints_wildcard_no_overlap_different_files() {
        // file1::* should NOT match file2::symbol
        let mut a = ResolvedFootprint::default();
        a.symbols.insert("src/auth.rs::*".to_string());

        let mut b = ResolvedFootprint::default();
        b.symbols.insert("src/user.rs::create".to_string());

        assert!(!footprints_overlap(&a, &b));
    }

    #[test]
    fn test_footprints_create_overlaps_modify() {
        // Creating a file should overlap with modifying symbols in that file
        let mut a = ResolvedFootprint::default();
        a.creates.insert("src/new_file.rs".to_string());

        let mut b = ResolvedFootprint::default();
        b.symbols.insert("src/new_file.rs::SomeStruct".to_string());

        assert!(footprints_overlap(&a, &b));
    }

    #[test]
    fn test_symbols_match_wildcards() {
        use validation::symbols_match;
        assert!(symbols_match("src/auth.rs::*", "src/auth.rs::login"));
        assert!(symbols_match("src/auth.rs::login", "src/auth.rs::*"));
        assert!(!symbols_match("src/auth.rs::*", "src/user.rs::login"));
        assert!(!symbols_match("src/auth.rs::login", "src/user.rs::*"));
    }

    #[test]
    fn test_symbols_match_bare_file_paths() {
        use validation::symbols_match;
        // Bare file path should be treated as file::*
        assert!(symbols_match("src/auth.rs", "src/auth.rs::login"));
        assert!(symbols_match("src/auth.rs::login", "src/auth.rs"));
        assert!(symbols_match("src/auth.rs", "src/auth.rs::*"));
        assert!(!symbols_match("src/auth.rs", "src/user.rs::login"));
        // Two bare paths for same file should match
        assert!(symbols_match("src/auth.rs", "src/auth.rs"));
    }

    // ========================================================================
    // SQLite Task Tests
    // ========================================================================

    fn setup_test_db() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        crate::db::init_db(Some(db_path.to_str().unwrap())).unwrap();
        dir
    }

    #[test]
    fn test_normalize_footprint_exact_symbol() {
        let footprint = TaskFootprint {
            modifies: vec!["src/auth.rs::AuthHandler".to_string()],
            creates: vec![],
        };

        let normalized = normalize_footprint(&footprint);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].pattern_type, "modifies");
        assert_eq!(normalized[0].file_path, "src/auth.rs");
        assert_eq!(normalized[0].symbol, "AuthHandler");
        assert!(!normalized[0].is_wildcard);
    }

    #[test]
    fn test_normalize_footprint_wildcard() {
        let footprint = TaskFootprint {
            modifies: vec!["src/jwt.rs::*".to_string()],
            creates: vec![],
        };

        let normalized = normalize_footprint(&footprint);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].pattern_type, "modifies");
        assert_eq!(normalized[0].file_path, "src/jwt.rs");
        assert_eq!(normalized[0].symbol, "");
        assert!(normalized[0].is_wildcard);
    }

    #[test]
    fn test_normalize_footprint_bare_file() {
        let footprint = TaskFootprint {
            modifies: vec!["src/config.rs".to_string()],
            creates: vec![],
        };

        let normalized = normalize_footprint(&footprint);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].file_path, "src/config.rs");
        assert_eq!(normalized[0].symbol, "");
        assert!(normalized[0].is_wildcard);
    }

    #[test]
    fn test_normalize_footprint_creates() {
        let footprint = TaskFootprint {
            modifies: vec![],
            creates: vec!["src/new_file.rs".to_string()],
        };

        let normalized = normalize_footprint(&footprint);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].pattern_type, "creates");
        assert_eq!(normalized[0].file_path, "src/new_file.rs");
        assert_eq!(normalized[0].symbol, "");
        assert!(normalized[0].is_wildcard);
    }

    #[test]
    fn test_normalize_footprint_nested_symbol() {
        // Nested symbols like file::Struct::method should preserve full symbol path
        let footprint = TaskFootprint {
            modifies: vec!["src/foo.rs::Foo::bar".to_string()],
            creates: vec![],
        };

        let normalized = normalize_footprint(&footprint);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].pattern_type, "modifies");
        assert_eq!(normalized[0].file_path, "src/foo.rs");
        assert_eq!(normalized[0].symbol, "Foo::bar"); // Full nested path preserved
        assert!(!normalized[0].is_wildcard);
    }

    #[test]
    fn test_normalize_footprint_deduplication() {
        // Duplicate patterns should be deduplicated
        let footprint = TaskFootprint {
            modifies: vec![
                "src/auth.rs::Handler".to_string(),
                "src/auth.rs::Handler".to_string(), // Duplicate
                "src/jwt.rs::*".to_string(),
                "src/jwt.rs::*".to_string(), // Duplicate wildcard
            ],
            creates: vec![
                "src/new.rs".to_string(),
                "src/new.rs".to_string(), // Duplicate create
            ],
        };

        let normalized = normalize_footprint(&footprint);
        assert_eq!(normalized.len(), 3); // Only 3 unique entries
    }

    #[test]
    fn test_normalize_footprint_malformed_empty_symbol() {
        // Malformed file:: (empty symbol after ::) should be treated as wildcard
        let footprint = TaskFootprint {
            modifies: vec!["src/foo.rs::".to_string()],
            creates: vec![],
        };

        let normalized = normalize_footprint(&footprint);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].pattern_type, "modifies");
        assert_eq!(normalized[0].file_path, "src/foo.rs");
        assert_eq!(normalized[0].symbol, ""); // Empty symbol
        assert!(normalized[0].is_wildcard); // Treated as wildcard
    }

    #[test]
    fn test_create_sqlite_task() {
        let _dir = setup_test_db();

        // Create epic first
        crate::epics::create_epic(crate::epics::CreateEpicInput {
            id: "TEST-EPIC".to_string(),
            title: "Test Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        })
        .unwrap();

        // Create task
        let input = CreateSqliteTaskInput {
            id: "TEST-001".to_string(),
            epic_id: "TEST-EPIC".to_string(),
            title: "Test Task".to_string(),
            description: Some("A test task".to_string()),
            priority: 3,
            depends_on: vec![],
            task_type: None,
            archetype: None,
            footprint: TaskFootprint::default(),
        };

        let task = create_sqlite_task(input).unwrap();
        assert_eq!(task.id, "TEST-001");
        assert_eq!(task.epic_id, "TEST-EPIC");
        assert_eq!(task.status, SqliteTaskStatus::Open);
        assert_eq!(task.priority, 3);

        crate::db::close_db();
    }

    #[test]
    fn test_claim_sqlite_task() {
        let _dir = setup_test_db();

        // Setup: create epic and task
        crate::epics::create_epic(crate::epics::CreateEpicInput {
            id: "CLAIM-EPIC".to_string(),
            title: "Claim Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        })
        .unwrap();

        create_sqlite_task(CreateSqliteTaskInput {
            id: "CLAIM-001".to_string(),
            epic_id: "CLAIM-EPIC".to_string(),
            title: "Claimable Task".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            task_type: None,
            archetype: None,
            footprint: TaskFootprint::default(),
        })
        .unwrap();

        // Claim the task
        let task = claim_sqlite_task("CLAIM-001", "agent-1").unwrap();
        assert_eq!(task.status, SqliteTaskStatus::InProgress);
        assert_eq!(task.claimed_by, Some("agent-1".to_string()));

        // Second claim should fail
        let result = claim_sqlite_task("CLAIM-001", "agent-2");
        assert!(result.is_err());

        crate::db::close_db();
    }

    #[test]
    fn test_claim_next_sqlite_task() {
        let _dir = setup_test_db();

        // Setup: create epic and tasks
        crate::epics::create_epic(crate::epics::CreateEpicInput {
            id: "NEXT-EPIC".to_string(),
            title: "Next Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        })
        .unwrap();

        // Create tasks with different priorities
        create_sqlite_task(CreateSqliteTaskInput {
            id: "NEXT-LOW".to_string(),
            epic_id: "NEXT-EPIC".to_string(),
            title: "Low Priority".to_string(),
            description: None,
            priority: 10, // Lower priority (higher number)
            depends_on: vec![],
            task_type: None,
            archetype: None,
            footprint: TaskFootprint::default(),
        })
        .unwrap();

        create_sqlite_task(CreateSqliteTaskInput {
            id: "NEXT-HIGH".to_string(),
            epic_id: "NEXT-EPIC".to_string(),
            title: "High Priority".to_string(),
            description: None,
            priority: 1, // Higher priority (lower number)
            depends_on: vec![],
            task_type: None,
            archetype: None,
            footprint: TaskFootprint::default(),
        })
        .unwrap();

        // Should claim the higher priority task first
        let task = claim_next_sqlite_task("agent-1").unwrap();
        assert!(task.is_some());
        let task = task.unwrap();
        assert_eq!(task.id, "NEXT-HIGH");

        crate::db::close_db();
    }

    #[test]
    fn test_orchestrator_lease_acquire_and_release() {
        let _dir = setup_test_db();

        assert!(try_acquire_orchestrator_lease("run-a", ORCHESTRATOR_LEASE_TTL_MS).unwrap());
        assert!(!try_acquire_orchestrator_lease("run-b", ORCHESTRATOR_LEASE_TTL_MS).unwrap());

        let lease = get_orchestrator_lease()
            .unwrap()
            .expect("lease should exist");
        assert_eq!(lease.holder_id, "run-a");

        release_orchestrator_lease("run-a").unwrap();
        assert!(try_acquire_orchestrator_lease("run-b", ORCHESTRATOR_LEASE_TTL_MS).unwrap());

        crate::db::close_db();
    }

    #[test]
    fn test_orchestrator_lease_takeover_after_expiry() {
        let _dir = setup_test_db();

        assert!(try_acquire_orchestrator_lease("run-a", ORCHESTRATOR_LEASE_TTL_MS).unwrap());

        let expired = chrono::Utc::now().timestamp_millis() - 1;
        with_db(|conn| {
            conn.execute(
                "UPDATE orchestrator_leases
                 SET lease_expires_at = ?1
                 WHERE lease_name = 'global'",
                [expired],
            )?;
            Ok(())
        })
        .unwrap();

        assert!(try_acquire_orchestrator_lease("run-b", ORCHESTRATOR_LEASE_TTL_MS).unwrap());
        let lease = get_orchestrator_lease()
            .unwrap()
            .expect("lease should exist");
        assert_eq!(lease.holder_id, "run-b");

        crate::db::close_db();
    }

    #[test]
    fn test_sqlite_task_dependencies() {
        let _dir = setup_test_db();

        // Setup
        crate::epics::create_epic(crate::epics::CreateEpicInput {
            id: "DEP-EPIC".to_string(),
            title: "Dep Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        })
        .unwrap();

        // Create first task
        create_sqlite_task(CreateSqliteTaskInput {
            id: "DEP-001".to_string(),
            epic_id: "DEP-EPIC".to_string(),
            title: "First Task".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            task_type: None,
            archetype: None,
            footprint: TaskFootprint::default(),
        })
        .unwrap();

        // Create second task depending on first
        create_sqlite_task(CreateSqliteTaskInput {
            id: "DEP-002".to_string(),
            epic_id: "DEP-EPIC".to_string(),
            title: "Second Task".to_string(),
            description: None,
            priority: 5,
            depends_on: vec!["DEP-001".to_string()],
            task_type: None,
            archetype: None,
            footprint: TaskFootprint::default(),
        })
        .unwrap();

        // Second task should not be claimable (dep not satisfied)
        let result = claim_sqlite_task("DEP-002", "agent-1");
        assert!(result.is_err());

        // Claim and release first task
        claim_sqlite_task("DEP-001", "agent-1").unwrap();
        release_sqlite_task("DEP-001", "agent-1").unwrap();

        // Now second task should be claimable
        let task = claim_sqlite_task("DEP-002", "agent-2").unwrap();
        assert_eq!(task.id, "DEP-002");

        crate::db::close_db();
    }

    #[test]
    fn test_sqlite_task_footprint_collision() {
        let _dir = setup_test_db();

        // Setup
        crate::epics::create_epic(crate::epics::CreateEpicInput {
            id: "FP-EPIC".to_string(),
            title: "Footprint Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        })
        .unwrap();

        // Create tasks with overlapping footprints
        create_sqlite_task(CreateSqliteTaskInput {
            id: "FP-001".to_string(),
            epic_id: "FP-EPIC".to_string(),
            title: "First Modifier".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            task_type: None,
            archetype: None,
            footprint: TaskFootprint {
                modifies: vec!["src/auth.rs::Handler".to_string()],
                creates: vec![],
            },
        })
        .unwrap();

        create_sqlite_task(CreateSqliteTaskInput {
            id: "FP-002".to_string(),
            epic_id: "FP-EPIC".to_string(),
            title: "Second Modifier".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            task_type: None,
            archetype: None,
            footprint: TaskFootprint {
                modifies: vec!["src/auth.rs::Handler".to_string()], // Same symbol
                creates: vec![],
            },
        })
        .unwrap();

        // Claim first task
        claim_sqlite_task("FP-001", "agent-1").unwrap();

        // Second task should not be claimable (footprint collision)
        let result = claim_sqlite_task("FP-002", "agent-2");
        assert!(result.is_err());

        // Release first task
        release_sqlite_task("FP-001", "agent-1").unwrap();

        // Now second task should be claimable
        let task = claim_sqlite_task("FP-002", "agent-2").unwrap();
        assert_eq!(task.id, "FP-002");

        crate::db::close_db();
    }

    // ========================================================================
    // Import Tests
    // ========================================================================

    fn setup_test_workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let bacchus_dir = dir.path().join(".bacchus");
        std::fs::create_dir_all(&bacchus_dir).unwrap();

        let tasks_yaml = r#"
version: 1
tasks:
  - id: YAML-001
    title: YAML Task 1
    status: open
    priority: 1
    depends_on: []
  - id: YAML-002
    title: YAML Task 2
    status: open
    priority: 2
    depends_on: [YAML-001]
"#;
        std::fs::write(bacchus_dir.join("tasks.yaml"), tasks_yaml).unwrap();
        dir
    }

    #[test]
    fn test_import_yaml_tasks_basic() {
        let _dir = setup_test_db();
        let workspace = setup_test_workspace();

        // Import tasks
        let result = import_yaml_tasks(workspace.path(), Some("TEST-EPIC")).unwrap();

        assert_eq!(result.imported, 2);
        assert_eq!(result.skipped, 0);
        assert!(result.imported_ids.contains(&"YAML-001".to_string()));
        assert!(result.imported_ids.contains(&"YAML-002".to_string()));
        assert_eq!(result.epic_id, "TEST-EPIC");

        // Verify tasks are in SQLite
        let task1 = get_sqlite_task("YAML-001").unwrap();
        assert_eq!(task1.title, "YAML Task 1");
        assert_eq!(task1.epic_id, "TEST-EPIC");

        crate::db::close_db();
    }

    #[test]
    fn test_import_yaml_tasks_idempotent() {
        let _dir = setup_test_db();
        let workspace = setup_test_workspace();

        // First import
        let result1 = import_yaml_tasks(workspace.path(), Some("IDEM-EPIC")).unwrap();
        assert_eq!(result1.imported, 2);

        // Second import should skip all
        let result2 = import_yaml_tasks(workspace.path(), Some("IDEM-EPIC")).unwrap();
        assert_eq!(result2.imported, 0);
        assert_eq!(result2.skipped, 2);
        assert!(result2.skipped_ids.contains(&"YAML-001".to_string()));
        assert!(result2.skipped_ids.contains(&"YAML-002".to_string()));

        crate::db::close_db();
    }

    #[test]
    fn test_import_yaml_tasks_auto_epic_id() {
        let _dir = setup_test_db();
        let workspace = setup_test_workspace();

        // Import without epic ID - should auto-generate from task prefix
        let result = import_yaml_tasks(workspace.path(), None).unwrap();

        assert_eq!(result.imported, 2);
        assert_eq!(result.epic_id, "YAML-IMPORT"); // Auto-generated from "YAML-001"

        crate::db::close_db();
    }

    #[test]
    fn test_import_yaml_tasks_with_deps() {
        let _dir = setup_test_db();
        let workspace = setup_test_workspace();

        // Import tasks with dependencies
        let result = import_yaml_tasks(workspace.path(), Some("DEP-IMP-EPIC")).unwrap();
        assert_eq!(result.imported, 2);

        // Verify dependency was preserved
        // YAML-002 depends on YAML-001, so it shouldn't be ready
        let ready_tasks = get_ready_sqlite_tasks(Some("DEP-IMP-EPIC")).unwrap();
        assert_eq!(ready_tasks.len(), 1);
        assert_eq!(ready_tasks[0].id, "YAML-001");

        crate::db::close_db();
    }

    #[test]
    fn test_import_yaml_tasks_no_file() {
        let _dir = setup_test_db();
        let workspace = tempfile::tempdir().unwrap();

        // No tasks.yaml file
        let result = import_yaml_tasks(workspace.path(), Some("EMPTY-EPIC"));
        assert!(matches!(result, Err(TasksError::NoTasksFile(_))));

        crate::db::close_db();
    }
}
