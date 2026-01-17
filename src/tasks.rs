//! Task management module
//!
//! Built-in YAML-based task management for multi-agent coordination.
//! Source of truth: `.bacchus/tasks.yaml`

use crate::db::with_db;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use thiserror::Error;

// ============================================================================
// Types
// ============================================================================

/// Task footprint - symbols this task modifies/creates
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskFootprint {
    /// Symbols this task will modify (e.g., "src/auth/handler.rs::AuthHandler", "src/auth/jwt.rs::*")
    #[serde(default)]
    pub modifies: Vec<String>,
    /// New files this task will create (virtual footprint)
    #[serde(default)]
    pub creates: Vec<String>,
}

/// A task in the tasks.yaml file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Lower = higher priority (default: 5)
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// open | in_progress | blocked | closed
    #[serde(default = "default_status")]
    pub status: String,
    /// Task IDs that must be closed first
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Symbol-level footprint for collision detection
    #[serde(default)]
    pub footprint: TaskFootprint,
}

fn default_priority() -> i32 {
    5
}

fn default_status() -> String {
    "open".to_string()
}

/// The tasks.yaml schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksFile {
    #[serde(default = "default_version")]
    pub version: i32,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

fn default_version() -> i32 {
    1
}

impl Default for TasksFile {
    fn default() -> Self {
        Self {
            version: 1,
            tasks: Vec::new(),
        }
    }
}

/// Resolved footprint with actual symbol matches
#[derive(Debug, Clone, Default)]
pub struct ResolvedFootprint {
    /// Matched symbol fq_names
    pub symbols: HashSet<String>,
    /// File paths being created
    pub creates: HashSet<String>,
}

/// Errors that can occur when working with tasks
#[derive(Debug, Error)]
pub enum TasksError {
    #[error("Failed to read tasks file: {0}")]
    ReadError(String),

    #[error("Failed to parse tasks file: {0}")]
    ParseError(String),

    #[error("Failed to write tasks file: {0}")]
    WriteError(String),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Invalid status: {0}")]
    InvalidStatus(String),

    #[error("Database error: {0}")]
    DbError(String),
}

// ============================================================================
// File Operations
// ============================================================================

/// Get the path to the tasks.yaml file
pub fn tasks_file_path(workspace_root: &Path) -> std::path::PathBuf {
    workspace_root.join(".bacchus/tasks.yaml")
}

/// Load tasks from the YAML file
pub fn load_tasks(workspace_root: &Path) -> Result<Vec<Task>, TasksError> {
    let path = tasks_file_path(workspace_root);

    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| TasksError::ReadError(e.to_string()))?;

    let tasks_file: TasksFile = serde_yaml::from_str(&content)
        .map_err(|e| TasksError::ParseError(e.to_string()))?;

    Ok(tasks_file.tasks)
}

/// Save tasks to the YAML file
pub fn save_tasks(workspace_root: &Path, tasks: &[Task]) -> Result<(), TasksError> {
    let path = tasks_file_path(workspace_root);

    // Ensure .bacchus directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TasksError::WriteError(e.to_string()))?;
    }

    let tasks_file = TasksFile {
        version: 1,
        tasks: tasks.to_vec(),
    };

    let content = serde_yaml::to_string(&tasks_file)
        .map_err(|e| TasksError::WriteError(e.to_string()))?;

    std::fs::write(&path, content)
        .map_err(|e| TasksError::WriteError(e.to_string()))?;

    Ok(())
}

// ============================================================================
// Public API
// ============================================================================

/// Get a specific task by ID
pub fn get_task(workspace_root: &Path, task_id: &str) -> Result<Task, TasksError> {
    let tasks = load_tasks(workspace_root)?;

    tasks
        .into_iter()
        .find(|t| t.id == task_id)
        .ok_or_else(|| TasksError::TaskNotFound(task_id.to_string()))
}

/// Check if a specific task is ready to work on
pub fn is_task_ready(workspace_root: &Path, task_id: &str) -> Result<bool, TasksError> {
    let ready = get_ready_tasks(workspace_root)?;
    Ok(ready.iter().any(|t| t.id == task_id))
}

/// Get tasks that are in progress
pub fn get_in_progress_tasks(workspace_root: &Path) -> Result<Vec<Task>, TasksError> {
    let tasks = load_tasks(workspace_root)?;
    Ok(tasks.into_iter().filter(|t| t.status == "in_progress").collect())
}

/// Get tasks that are ready to work on
///
/// A task is ready when ALL conditions are met:
/// 1. status == "open"
/// 2. All tasks in depends_on have status == "closed"
/// 3. No active claim has overlapping footprint
pub fn get_ready_tasks(workspace_root: &Path) -> Result<Vec<Task>, TasksError> {
    let tasks = load_tasks(workspace_root)?;

    // Build a set of closed task IDs for dependency checking
    let closed_ids: HashSet<String> = tasks
        .iter()
        .filter(|t| t.status == "closed")
        .map(|t| t.id.clone())
        .collect();

    // Get active footprints from claims
    let active_footprints = get_active_footprints().unwrap_or_default();

    let mut ready: Vec<Task> = tasks
        .into_iter()
        .filter(|task| {
            // Condition 1: Must be open
            if task.status != "open" {
                return false;
            }

            // Condition 2: All dependencies must be closed
            if !task.depends_on.iter().all(|dep| closed_ids.contains(dep)) {
                return false;
            }

            // Condition 3: No footprint collision with active claims
            let task_footprint = resolve_footprint(&task.footprint);
            if footprints_overlap(&task_footprint, &active_footprints) {
                return false;
            }

            true
        })
        .collect();

    // Sort by priority (lower = higher priority)
    ready.sort_by_key(|t| t.priority);

    Ok(ready)
}

/// Update a task's status in the YAML file
pub fn update_task_status(workspace_root: &Path, task_id: &str, status: &str) -> Result<(), TasksError> {
    // Validate status
    let valid_statuses = ["open", "in_progress", "blocked", "closed"];
    if !valid_statuses.contains(&status) {
        return Err(TasksError::InvalidStatus(status.to_string()));
    }

    let mut tasks = load_tasks(workspace_root)?;

    let task = tasks
        .iter_mut()
        .find(|t| t.id == task_id)
        .ok_or_else(|| TasksError::TaskNotFound(task_id.to_string()))?;

    task.status = status.to_string();

    save_tasks(workspace_root, &tasks)
}

// ============================================================================
// Footprint Helpers
// ============================================================================

/// Resolve a footprint pattern against the symbol index
///
/// Supports:
/// - Exact symbol: "src/auth/handler.rs::AuthHandler"
/// - File glob: "src/auth/jwt.rs::*" (all symbols in file)
pub fn resolve_footprint(footprint: &TaskFootprint) -> ResolvedFootprint {
    let mut resolved = ResolvedFootprint::default();

    // Add creates as-is
    for path in &footprint.creates {
        resolved.creates.insert(path.clone());
    }

    // Resolve modifies patterns against symbol index
    for pattern in &footprint.modifies {
        if let Some((file_part, symbol_part)) = pattern.rsplit_once("::") {
            if symbol_part == "*" {
                // File glob - get all symbols in file
                if let Ok(symbols) = get_symbols_in_file(file_part) {
                    for sym in symbols {
                        resolved.symbols.insert(sym);
                    }
                }
                // Even if no symbols found, track the file pattern
                resolved.symbols.insert(pattern.clone());
            } else {
                // Exact symbol
                resolved.symbols.insert(pattern.clone());
            }
        } else {
            // No :: - treat as file path, get all symbols
            if let Ok(symbols) = get_symbols_in_file(pattern) {
                for sym in symbols {
                    resolved.symbols.insert(sym);
                }
            }
            resolved.symbols.insert(format!("{}::*", pattern));
        }
    }

    resolved
}

/// Check if two footprints overlap
pub fn footprints_overlap(a: &ResolvedFootprint, b: &ResolvedFootprint) -> bool {
    // Check symbol overlap
    if !a.symbols.is_disjoint(&b.symbols) {
        return true;
    }

    // Check creates overlap
    if !a.creates.is_disjoint(&b.creates) {
        return true;
    }

    // Check if any created file overlaps with modified symbols
    for create_path in &a.creates {
        for sym in &b.symbols {
            if sym.starts_with(create_path) || sym.starts_with(&format!("{}::", create_path)) {
                return true;
            }
        }
    }
    for create_path in &b.creates {
        for sym in &a.symbols {
            if sym.starts_with(create_path) || sym.starts_with(&format!("{}::", create_path)) {
                return true;
            }
        }
    }

    false
}

/// Get symbols in a specific file from the index
fn get_symbols_in_file(file_path: &str) -> Result<Vec<String>, TasksError> {
    with_db(|conn| {
        let mut stmt = conn
            .prepare("SELECT fq_name FROM symbols WHERE file = ?1")?;

        let symbols: Vec<String> = stmt
            .query_map([file_path], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(symbols)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Get active footprints from all current claims
fn get_active_footprints() -> Result<ResolvedFootprint, TasksError> {
    with_db(|conn| {
        let mut resolved = ResolvedFootprint::default();

        // Query active_footprints table
        let mut stmt = conn
            .prepare("SELECT pattern, pattern_type, resolved_symbols FROM active_footprints")?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;

        for row_result in rows {
            if let Ok((pattern, pattern_type, resolved_symbols)) = row_result {
                match pattern_type.as_str() {
                    "modifies" => {
                        // Add the pattern itself
                        resolved.symbols.insert(pattern);
                        // Add resolved symbols if available
                        if let Some(json) = resolved_symbols {
                            if let Ok(symbols) = serde_json::from_str::<Vec<String>>(&json) {
                                for sym in symbols {
                                    resolved.symbols.insert(sym);
                                }
                            }
                        }
                    }
                    "creates" => {
                        resolved.creates.insert(pattern);
                    }
                    _ => {}
                }
            }
        }

        Ok(resolved)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

// ============================================================================
// Active Footprint Management
// ============================================================================

/// Store footprints for an active claim
pub fn store_active_footprints(task_id: &str, footprint: &TaskFootprint) -> Result<(), TasksError> {
    with_db(|conn| {
        // Store modifies patterns
        for pattern in &footprint.modifies {
            // Resolve symbols for this pattern
            let resolved: Vec<String> = if let Some((file_part, symbol_part)) = pattern.rsplit_once("::") {
                if symbol_part == "*" {
                    get_symbols_in_file(file_part).unwrap_or_default()
                } else {
                    vec![pattern.clone()]
                }
            } else {
                get_symbols_in_file(pattern).unwrap_or_default()
            };

            let resolved_json = serde_json::to_string(&resolved).ok();

            conn.execute(
                "INSERT OR REPLACE INTO active_footprints (task_id, pattern, pattern_type, resolved_symbols) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![task_id, pattern, "modifies", resolved_json],
            )?;
        }

        // Store creates patterns
        for pattern in &footprint.creates {
            conn.execute(
                "INSERT OR REPLACE INTO active_footprints (task_id, pattern, pattern_type, resolved_symbols) VALUES (?1, ?2, ?3, NULL)",
                rusqlite::params![task_id, pattern, "creates"],
            )?;
        }

        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Clear footprints for a released claim
pub fn clear_active_footprints(task_id: &str) -> Result<(), TasksError> {
    with_db(|conn| {
        conn.execute(
            "DELETE FROM active_footprints WHERE task_id = ?1",
            [task_id],
        )?;
        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

// ============================================================================
// Validation
// ============================================================================

/// Validation result for a single task
#[derive(Debug, Serialize, Deserialize)]
pub struct TaskValidation {
    pub task_id: String,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Validate tasks against symbol index
pub fn validate_tasks(workspace_root: &Path) -> Result<Vec<TaskValidation>, TasksError> {
    let tasks = load_tasks(workspace_root)?;
    let task_ids: HashSet<_> = tasks.iter().map(|t| t.id.as_str()).collect();

    let mut validations = Vec::new();

    for task in &tasks {
        let mut warnings = Vec::new();
        let mut errors = Vec::new();

        // Check dependencies exist
        for dep in &task.depends_on {
            if !task_ids.contains(dep.as_str()) {
                errors.push(format!("Unknown dependency: {}", dep));
            }
        }

        // Check circular dependencies (simple check)
        if task.depends_on.contains(&task.id) {
            errors.push("Task depends on itself".to_string());
        }

        // Validate footprint symbols exist in index
        for pattern in &task.footprint.modifies {
            if let Some((file_part, symbol_part)) = pattern.rsplit_once("::") {
                if symbol_part != "*" {
                    // Check if exact symbol exists
                    let exists = with_db(|conn| {
                        let result = conn.query_row(
                            "SELECT 1 FROM symbols WHERE fq_name = ?1",
                            [pattern],
                            |_| Ok(true),
                        ).unwrap_or(false);
                        Ok(result)
                    }).unwrap_or(false);

                    if !exists {
                        warnings.push(format!("Symbol not in index (may be new): {}", pattern));
                    }
                } else {
                    // Check if file has any symbols
                    let count = with_db(|conn| {
                        let result = conn.query_row(
                            "SELECT COUNT(*) FROM symbols WHERE file = ?1",
                            [file_part],
                            |row| row.get::<_, i32>(0),
                        ).unwrap_or(0);
                        Ok(result)
                    }).unwrap_or(0);

                    if count == 0 {
                        warnings.push(format!("No indexed symbols in file (may be new): {}", file_part));
                    }
                }
            }
        }

        validations.push(TaskValidation {
            task_id: task.id.clone(),
            warnings,
            errors,
        });
    }

    Ok(validations)
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
"#.to_string()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
}
