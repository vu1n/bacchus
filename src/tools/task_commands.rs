//! Task CLI commands
//!
//! Provides commands for listing, showing, validating, and initializing tasks.

use crate::tasks::{self, Task, TaskValidation};
use serde::{Deserialize, Serialize};
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
pub struct TaskAddOutput {
    pub success: bool,
    pub task_id: String,
    pub message: String,
}

// ============================================================================
// Commands
// ============================================================================

/// List tasks with optional filters
pub fn list_tasks(
    workspace_root: &Path,
    status_filter: Option<&str>,
    ready_only: bool,
) -> Result<TaskListOutput, String> {
    let all_tasks = tasks::load_tasks(workspace_root)
        .map_err(|e| e.to_string())?;

    let ready_ids: std::collections::HashSet<String> = tasks::get_ready_tasks(workspace_root)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|t| t.id)
        .collect();

    let total = all_tasks.len();

    let filtered_tasks: Vec<TaskSummary> = all_tasks
        .into_iter()
        .filter(|t| {
            // Filter by status if specified
            if let Some(status) = status_filter {
                if t.status != status {
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
            TaskSummary {
                id: t.id,
                title: t.title,
                status: t.status,
                priority: t.priority,
                depends_on: t.depends_on,
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
pub fn show_task(workspace_root: &Path, task_id: &str) -> Result<TaskShowOutput, String> {
    let task = tasks::get_task(workspace_root, task_id)
        .map_err(|e| e.to_string())?;

    let all_tasks = tasks::load_tasks(workspace_root)
        .map_err(|e| e.to_string())?;

    // Build set of closed task IDs
    let closed_ids: std::collections::HashSet<_> = all_tasks
        .iter()
        .filter(|t| t.status == "closed")
        .map(|t| t.id.as_str())
        .collect();

    // Find blocking dependencies (not closed)
    let blocking_deps: Vec<String> = task
        .depends_on
        .iter()
        .filter(|dep| !closed_ids.contains(dep.as_str()))
        .cloned()
        .collect();

    // Check if task is ready
    let is_ready = tasks::is_task_ready(workspace_root, task_id)
        .unwrap_or(false);

    // TODO: Add footprint conflict detection
    let footprint_conflicts = Vec::new();

    Ok(TaskShowOutput {
        task,
        is_ready,
        blocking_deps,
        footprint_conflicts,
    })
}

/// Validate tasks against the symbol index
pub fn validate_tasks(workspace_root: &Path) -> Result<TaskValidateOutput, String> {
    let validations = tasks::validate_tasks(workspace_root)
        .map_err(|e| e.to_string())?;

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
        std::fs::create_dir_all(parent)
            .map_err(|e| e.to_string())?;
    }

    let template = tasks::generate_template();
    std::fs::write(&path, template)
        .map_err(|e| e.to_string())?;

    Ok(TaskInitOutput {
        success: true,
        path: path.to_string_lossy().to_string(),
        message: "Created tasks.yaml template".to_string(),
    })
}

/// Add a new task to tasks.yaml (atomic with file locking)
pub fn add_task(
    workspace_root: &Path,
    task_id: &str,
    title: &str,
    description: Option<&str>,
    priority: Option<i32>,
    depends_on: Vec<String>,
) -> Result<TaskAddOutput, String> {
    use fs2::FileExt;
    use std::io::Write;

    let path = tasks::tasks_file_path(workspace_root);

    // Ensure .bacchus directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Acquire lock for the entire read-modify-write cycle
    let lock_path = path.with_extension("yaml.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)
        .map_err(|e| format!("open lock: {}", e))?;

    lock_file
        .lock_exclusive()
        .map_err(|e| format!("lock: {}", e))?;

    // Read current tasks (within lock)
    let mut all_tasks = if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let tasks_file: tasks::TasksFile = serde_yaml::from_str(&content)
            .map_err(|e| e.to_string())?;
        tasks_file.tasks
    } else {
        Vec::new()
    };

    // Check for duplicate ID
    if all_tasks.iter().any(|t| t.id == task_id) {
        return Ok(TaskAddOutput {
            success: false,
            task_id: task_id.to_string(),
            message: format!("Task with ID {} already exists", task_id),
        });
    }

    // Create new task
    let new_task = Task {
        id: task_id.to_string(),
        title: title.to_string(),
        description: description.map(|s| s.to_string()),
        priority: priority.unwrap_or(5),
        status: "open".to_string(),
        depends_on,
        footprint: tasks::TaskFootprint::default(),
    };

    all_tasks.push(new_task);

    // Write back (within lock)
    let tasks_file = tasks::TasksFile {
        version: 1,
        tasks: all_tasks,
    };

    let content = serde_yaml::to_string(&tasks_file).map_err(|e| e.to_string())?;

    let temp_path = path.with_extension("yaml.tmp");
    let mut file = std::fs::File::create(&temp_path)
        .map_err(|e| format!("create temp: {}", e))?;

    file.write_all(content.as_bytes())
        .map_err(|e| format!("write: {}", e))?;

    file.sync_all().map_err(|e| format!("sync: {}", e))?;

    // Atomic rename (use remove + rename for Windows compatibility)
    #[cfg(windows)]
    let _ = std::fs::remove_file(&path);
    std::fs::rename(&temp_path, &path)
        .map_err(|e| format!("rename: {}", e))?;

    // Lock released when lock_file is dropped
    Ok(TaskAddOutput {
        success: true,
        task_id: task_id.to_string(),
        message: format!("Added task {}", task_id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_workspace() -> TempDir {
        let temp = TempDir::new().unwrap();
        let bacchus_dir = temp.path().join(".bacchus");
        std::fs::create_dir_all(&bacchus_dir).unwrap();

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
        temp
    }

    #[test]
    fn test_list_tasks() {
        let temp = setup_test_workspace();
        let result = list_tasks(temp.path(), None, false).unwrap();
        assert_eq!(result.total, 3);
        assert_eq!(result.filtered, 3);
    }

    #[test]
    fn test_list_tasks_status_filter() {
        let temp = setup_test_workspace();
        let result = list_tasks(temp.path(), Some("open"), false).unwrap();
        assert_eq!(result.filtered, 2);
    }

    #[test]
    fn test_show_task() {
        let temp = setup_test_workspace();
        let result = show_task(temp.path(), "TASK-002").unwrap();
        assert_eq!(result.task.id, "TASK-002");
        assert_eq!(result.blocking_deps, vec!["TASK-001"]);
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
        let temp = setup_test_workspace();
        let result = init_tasks(temp.path()).unwrap();
        assert!(!result.success);
        assert!(result.message.contains("already exists"));
    }
}
