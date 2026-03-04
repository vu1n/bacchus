//! YAML file operations for task management.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::params;

use crate::db::with_db;

use super::crud::create_sqlite_task;
use super::types::*;

// ============================================================================
// File Operations
// ============================================================================

/// Get the path to the tasks.yaml file
pub fn tasks_file_path(workspace_root: &Path) -> std::path::PathBuf {
    workspace_root.join(".bacchus/tasks.yaml")
}

/// Load tasks from the YAML file (read-only for import purposes)
pub fn load_tasks(workspace_root: &Path) -> Result<Vec<Task>, TasksError> {
    let path = tasks_file_path(workspace_root);

    if !path.exists() {
        return Ok(Vec::new());
    }

    let content =
        std::fs::read_to_string(&path).map_err(|e| TasksError::ReadError(e.to_string()))?;

    let tasks_file: TasksFile =
        serde_yaml::from_str(&content).map_err(|e| TasksError::ParseError(e.to_string()))?;

    Ok(tasks_file.tasks)
}

// ============================================================================
// Template Generation
// ============================================================================

/// Generate a template tasks.yaml content
pub fn generate_template() -> String {
    r#"# Bacchus Task Configuration
# See: https://github.com/vu1n/bacchus
version: 1

tasks:
  # Example task with dependencies and footprint
  # - id: AUTH-001
  #   title: "Implement user authentication"
  #   description: "Add JWT-based auth to the API"
  #   priority: 1                    # Lower = higher priority (default: 5)
  #   status: open                   # open | in_progress | blocked | closed
  #   depends_on: []                 # Task IDs that must be closed first
  #   footprint:
  #     modifies:                    # Symbols this task will change
  #       - "src/auth/handler.rs::AuthHandler"
  #       - "src/auth/jwt.rs::*"     # Glob: all symbols in file
  #     creates:                     # New files (virtual footprint)
  #       - "src/auth/middleware.rs"

  - id: EXAMPLE-001
    title: "Example task"
    description: "Replace this with your actual tasks"
    priority: 5
    status: open
    depends_on: []
    footprint:
      modifies: []
      creates: []
"#
    .to_string()
}

// ============================================================================
// Import Operations
// ============================================================================

/// Import tasks from YAML to SQLite
///
/// Reads `.bacchus/tasks.yaml` and creates corresponding tasks in SQLite.
/// Auto-creates an epic if none specified. Skips tasks that already exist (idempotent).
///
/// # Arguments
/// * `workspace_root` - Path to the workspace root
/// * `epic_id` - Optional epic ID. If None, auto-generates one from the first task prefix.
///
/// # Returns
/// ImportResult with counts of imported/skipped tasks
pub fn import_yaml_tasks(
    workspace_root: &Path,
    epic_id: Option<&str>,
) -> Result<ImportResult, TasksError> {
    let path = tasks_file_path(workspace_root);

    if !path.exists() {
        return Err(TasksError::NoTasksFile(path.to_string_lossy().to_string()));
    }

    let yaml_tasks = load_tasks(workspace_root)?;

    if yaml_tasks.is_empty() {
        return Ok(ImportResult {
            imported: 0,
            skipped: 0,
            imported_ids: vec![],
            skipped_ids: vec![],
            epic_id: epic_id.unwrap_or("").to_string(),
            warnings: vec!["No tasks found in YAML file".to_string()],
        });
    }

    // Determine epic ID
    let epic_id = match epic_id {
        Some(id) => id.to_string(),
        None => {
            // Auto-generate from first task ID prefix (e.g., "AUTH-001" -> "AUTH-IMPORT")
            let first_task = &yaml_tasks[0];
            if let Some(prefix) = first_task.id.split('-').next() {
                format!("{}-IMPORT", prefix)
            } else {
                "YAML-IMPORT".to_string()
            }
        }
    };

    // Ensure epic exists (create if needed)
    ensure_epic_exists(&epic_id)?;

    let mut imported = 0;
    let mut skipped = 0;
    let mut imported_ids = Vec::new();
    let mut skipped_ids = Vec::new();
    let mut warnings = Vec::new();

    // First pass: collect task IDs being imported to validate dependencies
    let yaml_task_ids: HashSet<String> = yaml_tasks.iter().map(|t| t.id.clone()).collect();

    // Process each task
    for task in &yaml_tasks {
        // Check if task already exists in SQLite
        let exists_in_sqlite = with_db(|conn| {
            Ok(conn
                .query_row("SELECT 1 FROM tasks WHERE id = ?1", [&task.id], |_| {
                    Ok(true)
                })
                .unwrap_or(false))
        })
        .unwrap_or(false);

        if exists_in_sqlite {
            skipped += 1;
            skipped_ids.push(task.id.clone());
            continue;
        }

        // Validate dependencies - they must either be in YAML or already in SQLite
        let mut valid_deps = Vec::new();
        for dep in &task.depends_on {
            if yaml_task_ids.contains(dep) {
                // Dep is in YAML, will be imported
                valid_deps.push(dep.clone());
            } else {
                // Check if dep exists in SQLite (same epic)
                let dep_exists = with_db(|conn| {
                    Ok(conn
                        .query_row(
                            "SELECT 1 FROM tasks WHERE id = ?1 AND epic_id = ?2 AND deleted_at IS NULL",
                            params![dep, &epic_id],
                            |_| Ok(true),
                        )
                        .unwrap_or(false))
                })
                .unwrap_or(false);

                if dep_exists {
                    valid_deps.push(dep.clone());
                } else {
                    warnings.push(format!(
                        "Task {}: dependency {} not found, skipping dependency",
                        task.id, dep
                    ));
                }
            }
        }

        // Create the SQLite task
        // Parse task_type from YAML if provided
        let task_type = task
            .task_type
            .as_ref()
            .map(|t| SqliteTaskType::from_str_lossy(t));
        let input = CreateSqliteTaskInput {
            id: task.id.clone(),
            epic_id: epic_id.clone(),
            title: task.title.clone(),
            description: task.description.clone(),
            priority: task.priority,
            task_type,
            archetype: task.archetype.clone(),
            depends_on: valid_deps,
            footprint: task.footprint.clone(),
        };

        match create_sqlite_task(input) {
            Ok(_) => {
                imported += 1;
                imported_ids.push(task.id.clone());
            }
            Err(e) => {
                warnings.push(format!("Failed to import task {}: {}", task.id, e));
                skipped += 1;
                skipped_ids.push(task.id.clone());
            }
        }
    }

    Ok(ImportResult {
        imported,
        skipped,
        imported_ids,
        skipped_ids,
        epic_id,
        warnings,
    })
}

/// Ensure an epic exists, creating it if needed
pub(crate) fn ensure_epic_exists(epic_id: &str) -> Result<(), TasksError> {
    let exists = with_db(|conn| {
        Ok(conn
            .query_row("SELECT 1 FROM epics WHERE id = ?1", [epic_id], |_| Ok(true))
            .unwrap_or(false))
    })
    .unwrap_or(false);

    if exists {
        return Ok(());
    }

    // Create the epic
    let now = chrono::Utc::now().timestamp_millis();
    with_db(|conn| {
        conn.execute(
            "INSERT INTO epics (id, title, description, status, created_by, created_at, updated_at)
             VALUES (?1, ?2, NULL, 'open', 'import', ?3, ?3)",
            params![epic_id, format!("Imported from YAML: {}", epic_id), now],
        )
    })
    .map_err(|e| TasksError::DbError(e.to_string()))?;

    Ok(())
}
