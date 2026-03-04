//! Task CLI commands
//!
//! Provides commands for listing, showing, validating, and initializing tasks.

use crate::quality::QualityCheck;
use crate::tasks::{self, Task, TaskValidation};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

// ============================================================================
// Output Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskListOutput {
    pub tasks: Vec<TaskSummary>,
    pub total: usize,
    pub filtered: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub priority: i32,
    pub depends_on: Vec<String>,
    pub is_ready: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskShowOutput {
    pub task: Task,
    pub is_ready: bool,
    pub blocking_deps: Vec<String>,
    pub footprint_conflicts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quality_results: Vec<QualityCheck>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskValidateOutput {
    pub valid: bool,
    pub tasks: Vec<TaskValidation>,
    pub error_count: usize,
    pub warning_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskInitOutput {
    pub success: bool,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskImportOutput {
    pub success: bool,
    pub imported: usize,
    pub skipped: usize,
    pub imported_ids: Vec<String>,
    pub skipped_ids: Vec<String>,
    pub epic_id: String,
    pub warnings: Vec<String>,
    pub message: String,
}

// ============================================================================
// Commands
// ============================================================================

/// List tasks with optional filters
pub fn list_tasks(
    _workspace_root: &Path,
    status_filter: Option<&str>,
    ready_only: bool,
) -> Result<TaskListOutput, String> {
    let all_tasks = tasks::list_sqlite_tasks(None, None, false).map_err(|e| e.to_string())?;

    let ready_ids: HashSet<String> = tasks::get_ready_sqlite_tasks(None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|t| t.id)
        .collect();

    let deps_map = tasks::queries::get_all_deps()?;

    let total = all_tasks.len();

    let filtered_tasks: Vec<TaskSummary> = all_tasks
        .into_iter()
        .filter(|t| {
            // Filter by status if specified
            if let Some(status) = status_filter {
                if t.status.as_str() != status {
                    return false;
                }
            }
            // Filter to ready-only if specified
            if ready_only && !ready_ids.contains(&t.id) {
                return false;
            }
            true
        })
        .map(|t| {
            let is_ready = ready_ids.contains(&t.id);
            let depends_on = deps_map.get(&t.id).cloned().unwrap_or_default();
            TaskSummary {
                id: t.id,
                title: t.title,
                status: t.status.as_str().to_string(),
                priority: t.priority,
                depends_on,
                is_ready,
            }
        })
        .collect();

    let filtered = filtered_tasks.len();

    Ok(TaskListOutput {
        tasks: filtered_tasks,
        total,
        filtered,
    })
}

/// Show details for a specific task
pub fn show_task(_workspace_root: &Path, task_id: &str) -> Result<TaskShowOutput, String> {
    let sqlite_task = tasks::get_sqlite_task(task_id).map_err(|e| e.to_string())?;

    let ready_ids: HashSet<String> = tasks::get_ready_sqlite_tasks(None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|t| t.id)
        .collect();

    let depends_on = tasks::queries::get_depends_on(task_id)?;
    let blocking_deps = tasks::queries::get_blocking_deps(task_id)?;
    let footprint = tasks::queries::get_task_footprint(task_id)?;
    let footprint_conflicts = tasks::queries::get_footprint_conflicts(task_id)?;

    let task = Task {
        id: sqlite_task.id,
        title: sqlite_task.title,
        description: sqlite_task.description,
        priority: sqlite_task.priority,
        status: sqlite_task.status.as_str().to_string(),
        task_type: Some(sqlite_task.task_type.as_str().to_string()),
        archetype: Some(sqlite_task.archetype),
        depends_on: depends_on.clone(),
        footprint,
    };

    let is_ready = ready_ids.contains(task_id);
    let quality_results = crate::quality::load_quality_results(task_id);

    Ok(TaskShowOutput {
        task,
        is_ready,
        blocking_deps,
        footprint_conflicts,
        quality_results,
    })
}

/// Validate tasks against the symbol index
pub fn validate_tasks(workspace_root: &Path) -> Result<TaskValidateOutput, String> {
    let validations = tasks::validate_tasks(workspace_root).map_err(|e| e.to_string())?;

    let error_count: usize = validations.iter().map(|v| v.errors.len()).sum();
    let warning_count: usize = validations.iter().map(|v| v.warnings.len()).sum();

    Ok(TaskValidateOutput {
        valid: error_count == 0,
        tasks: validations,
        error_count,
        warning_count,
    })
}

/// Initialize a tasks.yaml template
pub fn init_tasks(workspace_root: &Path) -> Result<TaskInitOutput, String> {
    let path = tasks::tasks_file_path(workspace_root);

    if path.exists() {
        return Ok(TaskInitOutput {
            success: false,
            path: path.to_string_lossy().to_string(),
            message: "tasks.yaml already exists".to_string(),
        });
    }

    // Ensure .bacchus directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let template = tasks::generate_template();
    std::fs::write(&path, template).map_err(|e| e.to_string())?;

    Ok(TaskInitOutput {
        success: true,
        path: path.to_string_lossy().to_string(),
        message: "Created tasks.yaml template".to_string(),
    })
}

/// Import tasks from YAML to SQLite
pub fn import_tasks(
    workspace_root: &Path,
    epic_id: Option<&str>,
) -> Result<TaskImportOutput, String> {
    match tasks::import_yaml_tasks(workspace_root, epic_id) {
        Ok(result) => {
            let message = if result.imported > 0 {
                format!(
                    "Imported {} tasks to epic '{}' ({} skipped)",
                    result.imported, result.epic_id, result.skipped
                )
            } else if result.skipped > 0 {
                format!(
                    "All {} tasks already exist in SQLite (epic '{}')",
                    result.skipped, result.epic_id
                )
            } else {
                "No tasks to import".to_string()
            };

            Ok(TaskImportOutput {
                success: true,
                imported: result.imported,
                skipped: result.skipped,
                imported_ids: result.imported_ids,
                skipped_ids: result.skipped_ids,
                epic_id: result.epic_id,
                warnings: result.warnings,
                message,
            })
        }
        Err(tasks::TasksError::NoTasksFile(path)) => Ok(TaskImportOutput {
            success: false,
            imported: 0,
            skipped: 0,
            imported_ids: vec![],
            skipped_ids: vec![],
            epic_id: epic_id.unwrap_or("").to_string(),
            warnings: vec![],
            message: format!("No tasks.yaml found at {}", path),
        }),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_workspace() -> TempDir {
        let temp = TempDir::new().unwrap();
        let bacchus_dir = temp.path().join(".bacchus");
        std::fs::create_dir_all(&bacchus_dir).unwrap();

        // Initialize database
        let db_path = bacchus_dir.join("test.db");
        crate::db::init_db(Some(db_path.to_str().unwrap())).unwrap();

        let tasks_yaml = r#"
version: 1
tasks:
  - id: TASK-001
    title: First task
    status: open
    priority: 1
    depends_on: []
  - id: TASK-002
    title: Second task
    status: open
    priority: 2
    depends_on: [TASK-001]
  - id: TASK-003
    title: Third task
    status: closed
    priority: 3
    depends_on: []
"#;
        std::fs::write(bacchus_dir.join("tasks.yaml"), tasks_yaml).unwrap();

        // Import tasks to SQLite
        let _ = tasks::import_yaml_tasks(temp.path(), Some("TEST-EPIC"));

        temp
    }

    fn cleanup_db() {
        crate::db::close_db();
    }

    #[test]
    fn test_list_tasks() {
        let temp = setup_test_workspace();
        let result = list_tasks(temp.path(), None, false).unwrap();
        assert_eq!(result.total, 3);
        assert_eq!(result.filtered, 3);
        cleanup_db();
    }

    #[test]
    fn test_list_tasks_status_filter() {
        let temp = setup_test_workspace();
        // All imported tasks start as 'open' regardless of YAML status
        let result = list_tasks(temp.path(), Some("open"), false).unwrap();
        assert_eq!(result.filtered, 3);
        cleanup_db();
    }

    #[test]
    fn test_show_task() {
        let temp = setup_test_workspace();
        let result = show_task(temp.path(), "TASK-002").unwrap();
        assert_eq!(result.task.id, "TASK-002");
        assert_eq!(result.blocking_deps, vec!["TASK-001"]);
        cleanup_db();
    }

    #[test]
    fn test_init_tasks_new() {
        let temp = TempDir::new().unwrap();
        let result = init_tasks(temp.path()).unwrap();
        assert!(result.success);
        assert!(temp.path().join(".bacchus/tasks.yaml").exists());
    }

    #[test]
    fn test_init_tasks_exists() {
        let temp = TempDir::new().unwrap();
        let bacchus_dir = temp.path().join(".bacchus");
        std::fs::create_dir_all(&bacchus_dir).unwrap();
        std::fs::write(bacchus_dir.join("tasks.yaml"), "test").unwrap();

        let result = init_tasks(temp.path()).unwrap();
        assert!(!result.success);
        assert!(result.message.contains("already exists"));
    }
}
