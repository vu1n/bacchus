//! Task management module
//!
//! Supports both YAML-based tasks (.bacchus/tasks.yaml) for backward compatibility
//! and SQLite-based tasks (tasks_v2 table) for hierarchical orchestration.
//!
//! ## Task Storage
//! - **YAML tasks**: Legacy format in `.bacchus/tasks.yaml` (no epic association)
//! - **SQLite tasks**: New format in `tasks_v2` table (must belong to an epic)
//!
//! ## Migration Path
//! 1. Existing YAML tasks continue to work
//! 2. New epics/tasks use SQLite via `epics::create_epic` and `create_sqlite_task`
//! 3. Future: `task migrate` command to move YAML tasks to SQLite

use crate::db::with_db;
use fs2::FileExt;
use rusqlite::params;
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

    #[error("Task already exists: {0}")]
    DuplicateTask(String),

    #[error("Invalid status: {0}")]
    InvalidStatus(String),

    #[error("Database error: {0}")]
    DbError(String),

    #[error("Task not ready: {0}")]
    NotReady(String),

    #[error("Epic not found: {0}")]
    EpicNotFound(String),
}

// ============================================================================
// SQLite Task Types (tasks_v2)
// ============================================================================

/// Task status for SQLite tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteTaskStatus {
    Draft,
    Open,
    InProgress,
    Blocked,
    Closed,
}

impl SqliteTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SqliteTaskStatus::Draft => "draft",
            SqliteTaskStatus::Open => "open",
            SqliteTaskStatus::InProgress => "in_progress",
            SqliteTaskStatus::Blocked => "blocked",
            SqliteTaskStatus::Closed => "closed",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, TasksError> {
        match s {
            "draft" => Ok(SqliteTaskStatus::Draft),
            "open" => Ok(SqliteTaskStatus::Open),
            "in_progress" => Ok(SqliteTaskStatus::InProgress),
            "blocked" => Ok(SqliteTaskStatus::Blocked),
            "closed" => Ok(SqliteTaskStatus::Closed),
            _ => Err(TasksError::InvalidStatus(s.to_string())),
        }
    }
}

impl std::fmt::Display for SqliteTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A task stored in SQLite (tasks_v2 table)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqliteTask {
    pub id: String,
    pub epic_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub priority: i32,
    pub status: SqliteTaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
}

/// Input for creating a new SQLite task
#[derive(Debug, Clone)]
pub struct CreateSqliteTaskInput {
    pub id: String,
    pub epic_id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: i32,
    pub depends_on: Vec<String>,
    pub footprint: TaskFootprint,
}

/// Normalized footprint entry for SQLite
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedFootprint {
    pub pattern_type: String, // "modifies" | "creates"
    pub file_path: String,
    pub symbol: String,
    pub is_wildcard: bool,
}

// ============================================================================
// File Operations
// ============================================================================

/// Get the path to the tasks.yaml file
pub fn tasks_file_path(workspace_root: &Path) -> std::path::PathBuf {
    workspace_root.join(".bacchus/tasks.yaml")
}

/// Get the path to the lock file for tasks.yaml
fn tasks_lock_path(workspace_root: &Path) -> std::path::PathBuf {
    workspace_root.join(".bacchus/tasks.yaml.lock")
}

/// Atomically write content to tasks.yaml with proper locking
///
/// Strategy:
/// - Unix: write to .tmp, rename over target (atomic)
/// - Windows: write to .tmp, rename target to .bak, rename .tmp to target, delete .bak
///   (minimizes window where file is missing, .bak allows recovery)
fn atomic_write_tasks(workspace_root: &Path, content: &str) -> Result<(), TasksError> {
    use std::io::Write;

    let path = tasks_file_path(workspace_root);
    let temp_path = path.with_extension("yaml.tmp");

    // Write to temp file
    let mut file = std::fs::File::create(&temp_path)
        .map_err(|e| TasksError::WriteError(format!("create temp: {}", e)))?;

    file.write_all(content.as_bytes())
        .map_err(|e| TasksError::WriteError(format!("write: {}", e)))?;

    file.sync_all()
        .map_err(|e| TasksError::WriteError(format!("sync: {}", e)))?;

    drop(file); // Close before rename

    // Platform-specific atomic replace
    #[cfg(unix)]
    {
        std::fs::rename(&temp_path, &path)
            .map_err(|e| TasksError::WriteError(format!("rename: {}", e)))?;
    }

    #[cfg(windows)]
    {
        let backup_path = path.with_extension("yaml.bak");

        // If target exists, rename to backup first
        if path.exists() {
            // Remove old backup if exists
            let _ = std::fs::remove_file(&backup_path);
            std::fs::rename(&path, &backup_path)
                .map_err(|e| TasksError::WriteError(format!("backup: {}", e)))?;
        }

        // Rename temp to target
        if let Err(e) = std::fs::rename(&temp_path, &path) {
            // Try to restore from backup
            if backup_path.exists() {
                let _ = std::fs::rename(&backup_path, &path);
            }
            return Err(TasksError::WriteError(format!("rename: {}", e)));
        }

        // Success - remove backup
        let _ = std::fs::remove_file(&backup_path);
    }

    Ok(())
}

/// Execute a function while holding an exclusive lock on tasks.yaml
/// Used for read-modify-write operations
fn with_exclusive_lock<F, T>(workspace_root: &Path, f: F) -> Result<T, TasksError>
where
    F: FnOnce() -> Result<T, TasksError>,
{
    let path = tasks_file_path(workspace_root);

    // Ensure .bacchus directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TasksError::WriteError(e.to_string()))?;
    }

    let lock_path = tasks_lock_path(workspace_root);
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)
        .map_err(|e| TasksError::WriteError(format!("open lock: {}", e)))?;

    lock_file
        .lock_exclusive()
        .map_err(|e| TasksError::WriteError(format!("lock: {}", e)))?;

    let result = f();

    // Lock released when lock_file is dropped
    drop(lock_file);

    result
}

/// Load tasks from the YAML file with shared locking
///
/// Uses shared lock to prevent reading while a write is in progress.
/// The lock is acquired before checking file existence to handle the
/// Windows atomic write window where the file is temporarily renamed.
pub fn load_tasks(workspace_root: &Path) -> Result<Vec<Task>, TasksError> {
    let path = tasks_file_path(workspace_root);
    let lock_path = tasks_lock_path(workspace_root);

    // Acquire shared lock BEFORE checking existence to avoid race with
    // Windows atomic writes (which rename to .bak temporarily)
    // This blocks until any exclusive lock (write) is released
    let _lock_file = if lock_path.exists() {
        let lf = std::fs::OpenOptions::new()
            .read(true)
            .open(&lock_path)
            .ok();
        if let Some(ref f) = lf {
            // Block waiting for shared lock - ensures we don't read during writes
            let _ = f.lock_shared();
        }
        lf
    } else {
        None
    };

    // Check existence AFTER acquiring lock
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| TasksError::ReadError(e.to_string()))?;

    let tasks_file: TasksFile = serde_yaml::from_str(&content)
        .map_err(|e| TasksError::ParseError(e.to_string()))?;

    // Lock released when _lock_file is dropped
    Ok(tasks_file.tasks)
}

/// Save tasks to the YAML file with atomic write and file locking
#[allow(dead_code)] // Public API, may be used by external callers
pub fn save_tasks(workspace_root: &Path, tasks: &[Task]) -> Result<(), TasksError> {
    with_exclusive_lock(workspace_root, || {
        let tasks_file = TasksFile {
            version: 1,
            tasks: tasks.to_vec(),
        };

        let content = serde_yaml::to_string(&tasks_file)
            .map_err(|e| TasksError::WriteError(e.to_string()))?;

        atomic_write_tasks(workspace_root, &content)
    })
}

/// Modify tasks with a locked read-modify-write cycle
///
/// The closure receives the current tasks and should return the modified list.
/// The entire operation is atomic with proper file locking.
pub fn modify_tasks<F>(workspace_root: &Path, f: F) -> Result<(), TasksError>
where
    F: FnOnce(Vec<Task>) -> Result<Vec<Task>, TasksError>,
{
    with_exclusive_lock(workspace_root, || {
        let path = tasks_file_path(workspace_root);

        // Read current tasks (within lock)
        let tasks = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| TasksError::ReadError(e.to_string()))?;
            let tasks_file: TasksFile = serde_yaml::from_str(&content)
                .map_err(|e| TasksError::ParseError(e.to_string()))?;
            tasks_file.tasks
        } else {
            Vec::new()
        };

        // Apply modification
        let modified_tasks = f(tasks)?;

        // Write back (within lock)
        let tasks_file = TasksFile {
            version: 1,
            tasks: modified_tasks,
        };

        let content = serde_yaml::to_string(&tasks_file)
            .map_err(|e| TasksError::WriteError(e.to_string()))?;

        atomic_write_tasks(workspace_root, &content)
    })
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

/// Update a task's status in the YAML file (atomic with file locking)
pub fn update_task_status(workspace_root: &Path, task_id: &str, status: &str) -> Result<(), TasksError> {
    // Validate status before acquiring lock
    let valid_statuses = ["open", "in_progress", "blocked", "closed"];
    if !valid_statuses.contains(&status) {
        return Err(TasksError::InvalidStatus(status.to_string()));
    }

    let task_id = task_id.to_string();
    let status = status.to_string();

    with_exclusive_lock(workspace_root, || {
        let path = tasks_file_path(workspace_root);

        // Read current tasks (within lock)
        let mut tasks = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| TasksError::ReadError(e.to_string()))?;
            let tasks_file: TasksFile = serde_yaml::from_str(&content)
                .map_err(|e| TasksError::ParseError(e.to_string()))?;
            tasks_file.tasks
        } else {
            Vec::new()
        };

        // Find and update task
        let task = tasks
            .iter_mut()
            .find(|t| t.id == task_id)
            .ok_or_else(|| TasksError::TaskNotFound(task_id.clone()))?;

        task.status = status.clone();

        // Write back (within lock)
        let tasks_file = TasksFile {
            version: 1,
            tasks,
        };

        let content = serde_yaml::to_string(&tasks_file)
            .map_err(|e| TasksError::WriteError(e.to_string()))?;

        atomic_write_tasks(workspace_root, &content)
    })
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
///
/// Handles wildcards: `file::*` overlaps with any `file::symbol`
pub fn footprints_overlap(a: &ResolvedFootprint, b: &ResolvedFootprint) -> bool {
    // Check exact symbol overlap
    if !a.symbols.is_disjoint(&b.symbols) {
        return true;
    }

    // Check wildcard overlap: file::* matches file::anything
    for sym_a in &a.symbols {
        for sym_b in &b.symbols {
            if symbols_match(sym_a, sym_b) {
                return true;
            }
        }
    }

    // Check creates overlap
    if !a.creates.is_disjoint(&b.creates) {
        return true;
    }

    // Check if any created file overlaps with modified symbols
    for create_path in &a.creates {
        for sym in &b.symbols {
            if symbols_match_file(create_path, sym) {
                return true;
            }
        }
    }
    for create_path in &b.creates {
        for sym in &a.symbols {
            if symbols_match_file(create_path, sym) {
                return true;
            }
        }
    }

    false
}

/// Check if two symbol patterns match (handles wildcards and bare file paths)
fn symbols_match(a: &str, b: &str) -> bool {
    // Exact match already handled by disjoint check
    if a == b {
        return true;
    }

    // Normalize bare file paths to file::* for comparison
    let norm_a = if a.contains("::") { a.to_string() } else { format!("{}::*", a) };
    let norm_b = if b.contains("::") { b.to_string() } else { format!("{}::*", b) };

    // Check if one is a wildcard for the other's file
    if let Some((file_a, sym_a)) = norm_a.rsplit_once("::") {
        if let Some((file_b, sym_b)) = norm_b.rsplit_once("::") {
            // Same file: wildcard matches any symbol
            if file_a == file_b && (sym_a == "*" || sym_b == "*") {
                return true;
            }
        }
    }

    false
}

/// Check if a file path matches a symbol pattern
fn symbols_match_file(file_path: &str, symbol: &str) -> bool {
    if let Some((file, _)) = symbol.rsplit_once("::") {
        file == file_path
    } else {
        // Symbol without :: is treated as file path
        symbol == file_path
    }
}

/// Get symbols in a specific file from the index
fn get_symbols_in_file(file_path: &str) -> Result<Vec<String>, TasksError> {
    with_db(|conn| get_symbols_in_file_with_conn(conn, file_path))
        .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Get symbols in a specific file (internal, takes connection to avoid deadlock)
fn get_symbols_in_file_with_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT fq_name FROM symbols WHERE file = ?1")?;

    let symbols: Vec<String> = stmt
        .query_map([file_path], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(symbols)
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
            // Normalize bare file paths to file::* for consistent matching
            let normalized_pattern = if pattern.contains("::") {
                pattern.clone()
            } else {
                format!("{}::*", pattern)
            };

            // Resolve symbols for this pattern (using conn to avoid deadlock)
            let resolved: Vec<String> = if let Some((file_part, symbol_part)) = normalized_pattern.rsplit_once("::") {
                if symbol_part == "*" {
                    get_symbols_in_file_with_conn(conn, file_part).unwrap_or_default()
                } else {
                    vec![normalized_pattern.clone()]
                }
            } else {
                // Shouldn't happen after normalization, but handle gracefully
                get_symbols_in_file_with_conn(conn, &normalized_pattern).unwrap_or_default()
            };

            let resolved_json = serde_json::to_string(&resolved).ok();

            conn.execute(
                "INSERT OR REPLACE INTO active_footprints (task_id, pattern, pattern_type, resolved_symbols) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![task_id, normalized_pattern, "modifies", resolved_json],
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
// SQLite Task Operations (tasks_v2)
// ============================================================================

/// Normalize a TaskFootprint into NormalizedFootprint entries for SQLite storage
///
/// Uses split_once (first ::) to correctly handle nested symbols like file::Struct::method
/// which becomes file_path="file", symbol="Struct::method"
pub fn normalize_footprint(footprint: &TaskFootprint) -> Vec<NormalizedFootprint> {
    let mut normalized = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for pattern in &footprint.modifies {
        // Use split_once (first ::) to handle nested symbols like file::Foo::bar
        // This gives file_path="file", symbol="Foo::bar"
        if let Some((file_path, symbol_part)) = pattern.split_once("::") {
            if symbol_part == "*" || symbol_part.is_empty() {
                // Wildcard: file::* or malformed file:: -> (file, "", is_wildcard=1)
                let key = ("modifies".to_string(), file_path.to_string(), String::new());
                if seen.insert(key) {
                    normalized.push(NormalizedFootprint {
                        pattern_type: "modifies".to_string(),
                        file_path: file_path.to_string(),
                        symbol: String::new(),
                        is_wildcard: true,
                    });
                }
            } else {
                // Exact symbol: file::Symbol or file::Struct::method -> (file, Symbol/Struct::method, is_wildcard=0)
                let key = ("modifies".to_string(), file_path.to_string(), symbol_part.to_string());
                if seen.insert(key) {
                    normalized.push(NormalizedFootprint {
                        pattern_type: "modifies".to_string(),
                        file_path: file_path.to_string(),
                        symbol: symbol_part.to_string(),
                        is_wildcard: false,
                    });
                }
            }
        } else {
            // Bare file path: file -> (file, "", is_wildcard=1)
            let key = ("modifies".to_string(), pattern.to_string(), String::new());
            if seen.insert(key) {
                normalized.push(NormalizedFootprint {
                    pattern_type: "modifies".to_string(),
                    file_path: pattern.to_string(),
                    symbol: String::new(),
                    is_wildcard: true,
                });
            }
        }
    }

    for path in &footprint.creates {
        // Creates are always wildcard (affects whole file)
        let key = ("creates".to_string(), path.to_string(), String::new());
        if seen.insert(key) {
            normalized.push(NormalizedFootprint {
                pattern_type: "creates".to_string(),
                file_path: path.to_string(),
                symbol: String::new(),
                is_wildcard: true,
            });
        }
    }

    normalized
}

/// Create a new SQLite task with dependencies and footprints
///
/// The task is created as 'draft' and atomically flipped to 'open' after
/// all dependencies and footprints are inserted.
pub fn create_sqlite_task(input: CreateSqliteTaskInput) -> Result<SqliteTask, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        // Check epic exists
        let epic_exists: bool = conn.query_row(
            "SELECT 1 FROM epics WHERE id = ?1",
            [&input.epic_id],
            |_| Ok(true),
        ).unwrap_or(false);

        if !epic_exists {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Epic not found: {}", input.epic_id)),
            ));
        }

        // Check for duplicate task ID
        let task_exists: bool = conn.query_row(
            "SELECT 1 FROM tasks_v2 WHERE id = ?1",
            [&input.id],
            |_| Ok(true),
        ).unwrap_or(false);

        if task_exists {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task already exists: {}", input.id)),
            ));
        }

        // Use savepoint for auto-rollback on error
        // SAVEPOINT works even if no transaction is active (SQLite auto-starts one)
        conn.execute("SAVEPOINT create_task", [])?;

        let result = (|| -> rusqlite::Result<i64> {
            // Insert task as draft
            conn.execute(
                "INSERT INTO tasks_v2 (id, epic_id, title, description, priority, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6, ?6)",
                params![input.id, input.epic_id, input.title, input.description, input.priority, now],
            )?;

            // Insert dependencies (trigger validates same-epic constraint)
            for dep_id in &input.depends_on {
                conn.execute(
                    "INSERT INTO task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
                    params![input.id, dep_id],
                )?;
            }

            // Insert normalized footprints
            let normalized = normalize_footprint(&input.footprint);
            for fp in &normalized {
                conn.execute(
                    "INSERT INTO task_footprints (task_id, pattern_type, file_path, symbol, is_wildcard)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![input.id, fp.pattern_type, fp.file_path, fp.symbol, fp.is_wildcard as i32],
                )?;
            }

            // Flip to open after all inserts succeed
            // Explicitly set updated_at to avoid staleness from trigger
            let flip_time = chrono::Utc::now().timestamp_millis();
            conn.execute(
                "UPDATE tasks_v2 SET status = 'open', updated_at = ?2 WHERE id = ?1",
                params![input.id, flip_time],
            )?;

            Ok(flip_time)
        })();

        match result {
            Ok(flip_time) => {
                conn.execute("RELEASE create_task", [])?;
                Ok(SqliteTask {
                    id: input.id,
                    epic_id: input.epic_id,
                    title: input.title,
                    description: input.description,
                    priority: input.priority,
                    status: SqliteTaskStatus::Open,
                    claimed_by: None,
                    claimed_at: None,
                    lease_expires_at: None,
                    heartbeat_at: None,
                    created_at: now,
                    updated_at: flip_time,
                    deleted_at: None,
                })
            }
            Err(e) => {
                // Rollback on any error
                let _ = conn.execute("ROLLBACK TO create_task", []);
                let _ = conn.execute("RELEASE create_task", []);
                Err(e)
            }
        }
    })
    .map_err(|e: rusqlite::Error| {
        let msg = e.to_string();
        if msg.contains("Epic not found") {
            TasksError::EpicNotFound(msg)
        } else if msg.contains("Task already exists") {
            TasksError::DuplicateTask(msg)
        } else if msg.contains("same epic") {
            TasksError::DbError("Dependencies must be within the same epic".to_string())
        } else {
            TasksError::DbError(msg)
        }
    })
}

/// Claim the next ready SQLite task atomically
///
/// Readiness = open + not deleted + deps satisfied + no footprint collision
/// Returns None if no ready tasks available.
pub fn claim_next_sqlite_task(agent_id: &str) -> Result<Option<SqliteTask>, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();
    let lease_expires = now + 300_000; // 5 minute lease

    with_db(|conn| {
        // Atomic claim with embedded readiness check
        // This is a complex query but it runs as a single atomic UPDATE
        conn.execute(
            r#"
            UPDATE tasks_v2
            SET status = 'in_progress',
                claimed_by = ?1,
                claimed_at = ?2,
                lease_expires_at = ?3,
                heartbeat_at = ?2,
                updated_at = ?2
            WHERE id = (
                SELECT t.id FROM tasks_v2 t
                WHERE t.status = 'open'
                  AND t.deleted_at IS NULL
                  -- All deps are closed OR deleted
                  AND NOT EXISTS (
                      SELECT 1 FROM task_dependencies td
                      JOIN tasks_v2 dep ON dep.id = td.depends_on
                      WHERE td.task_id = t.id
                        AND dep.status != 'closed'
                        AND dep.deleted_at IS NULL
                  )
                  -- No footprint overlap with OTHER in_progress tasks
                  AND NOT EXISTS (
                      SELECT 1 FROM task_footprints fp1
                      JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
                        AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
                      JOIN tasks_v2 other ON other.id = fp2.task_id
                      WHERE fp1.task_id = t.id
                        AND other.id != t.id
                        AND other.status = 'in_progress'
                        AND other.deleted_at IS NULL
                  )
                ORDER BY t.priority, t.created_at
                LIMIT 1
            )
            "#,
            params![agent_id, now, lease_expires],
        )?;

        // Check if we claimed anything
        let changes = conn.changes();
        if changes == 0 {
            return Ok(None);
        }

        // Fetch the claimed task
        let task = conn.query_row(
            "SELECT id, epic_id, title, description, priority, status, claimed_by, claimed_at,
                    lease_expires_at, heartbeat_at, created_at, updated_at, deleted_at
             FROM tasks_v2 WHERE claimed_by = ?1 AND status = 'in_progress'
             ORDER BY claimed_at DESC LIMIT 1",
            [agent_id],
            |row| {
                let status_str: String = row.get(5)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    claimed_by: row.get(6)?,
                    claimed_at: row.get(7)?,
                    lease_expires_at: row.get(8)?,
                    heartbeat_at: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    deleted_at: row.get(12)?,
                })
            },
        )?;

        Ok(Some(task))
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}

/// Claim a specific SQLite task atomically
///
/// Returns error if task is not ready (deps not satisfied, footprint collision, etc.)
pub fn claim_sqlite_task(task_id: &str, agent_id: &str) -> Result<SqliteTask, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();
    let lease_expires = now + 300_000; // 5 minute lease

    with_db(|conn| {
        // Atomic claim with readiness check
        let affected = conn.execute(
            r#"
            UPDATE tasks_v2
            SET status = 'in_progress',
                claimed_by = ?1,
                claimed_at = ?2,
                lease_expires_at = ?3,
                heartbeat_at = ?2,
                updated_at = ?2
            WHERE id = ?4
              AND status = 'open'
              AND deleted_at IS NULL
              -- All deps are closed OR deleted
              AND NOT EXISTS (
                  SELECT 1 FROM task_dependencies td
                  JOIN tasks_v2 dep ON dep.id = td.depends_on
                  WHERE td.task_id = ?4
                    AND dep.status != 'closed'
                    AND dep.deleted_at IS NULL
              )
              -- No footprint overlap with OTHER in_progress tasks
              AND NOT EXISTS (
                  SELECT 1 FROM task_footprints fp1
                  JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
                    AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
                  JOIN tasks_v2 other ON other.id = fp2.task_id
                  WHERE fp1.task_id = ?4
                    AND other.id != ?4
                    AND other.status = 'in_progress'
                    AND other.deleted_at IS NULL
              )
            "#,
            params![agent_id, now, lease_expires, task_id],
        )?;

        if affected == 0 {
            // Check why claim failed
            let task_status: Option<String> = conn.query_row(
                "SELECT status FROM tasks_v2 WHERE id = ?1 AND deleted_at IS NULL",
                [task_id],
                |row| row.get(0),
            ).ok();

            return Err(match task_status {
                None => rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(format!("Task not found: {}", task_id)),
                ),
                Some(s) if s != "open" => rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(format!("Task {} has status '{}', not 'open'", task_id, s)),
                ),
                _ => rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(format!("Task {} is not ready (deps or footprint collision)", task_id)),
                ),
            });
        }

        // Fetch the claimed task
        conn.query_row(
            "SELECT id, epic_id, title, description, priority, status, claimed_by, claimed_at,
                    lease_expires_at, heartbeat_at, created_at, updated_at, deleted_at
             FROM tasks_v2 WHERE id = ?1",
            [task_id],
            |row| {
                let status_str: String = row.get(5)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    claimed_by: row.get(6)?,
                    claimed_at: row.get(7)?,
                    lease_expires_at: row.get(8)?,
                    heartbeat_at: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    deleted_at: row.get(12)?,
                })
            },
        )
    })
    .map_err(|e: rusqlite::Error| {
        let msg = e.to_string();
        if msg.contains("Task not found") {
            TasksError::TaskNotFound(msg)
        } else if msg.contains("not ready") || msg.contains("not 'open'") {
            TasksError::NotReady(msg)
        } else {
            TasksError::DbError(msg)
        }
    })
}

/// Send a heartbeat for a claimed task (extends lease)
pub fn heartbeat_sqlite_task(task_id: &str, agent_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();
    let lease_expires = now + 300_000; // 5 minute lease

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks_v2
             SET heartbeat_at = ?1, lease_expires_at = ?2, updated_at = ?1
             WHERE id = ?3 AND claimed_by = ?4 AND status = 'in_progress' AND deleted_at IS NULL",
            params![now, lease_expires, task_id, agent_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task {} not owned by {} or not in_progress", task_id, agent_id)),
            ));
        }

        Ok(())
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}

/// Release a SQLite task (mark as closed, clear claim)
pub fn release_sqlite_task(task_id: &str, agent_id: &str) -> Result<SqliteTask, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks_v2
             SET status = 'closed',
                 claimed_by = NULL,
                 claimed_at = NULL,
                 lease_expires_at = NULL,
                 heartbeat_at = NULL,
                 updated_at = ?1
             WHERE id = ?2 AND claimed_by = ?3 AND status = 'in_progress'",
            params![now, task_id, agent_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task {} not owned by {} or not in_progress", task_id, agent_id)),
            ));
        }

        // Fetch the released task
        conn.query_row(
            "SELECT id, epic_id, title, description, priority, status, claimed_by, claimed_at,
                    lease_expires_at, heartbeat_at, created_at, updated_at, deleted_at
             FROM tasks_v2 WHERE id = ?1",
            [task_id],
            |row| {
                let status_str: String = row.get(5)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Closed),
                    claimed_by: row.get(6)?,
                    claimed_at: row.get(7)?,
                    lease_expires_at: row.get(8)?,
                    heartbeat_at: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    deleted_at: row.get(12)?,
                })
            },
        )
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}

/// Reclaim stale SQLite tasks (called by orchestrator)
///
/// Tasks with expired leases are reset to 'open' status.
/// Returns the number of tasks reclaimed.
pub fn reclaim_stale_sqlite_tasks() -> Result<usize, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks_v2
             SET status = 'open',
                 claimed_by = NULL,
                 claimed_at = NULL,
                 lease_expires_at = NULL,
                 heartbeat_at = NULL,
                 updated_at = ?1
             WHERE status = 'in_progress' AND lease_expires_at < ?1 AND deleted_at IS NULL",
            params![now],
        )?;

        Ok(affected)
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}

/// List SQLite tasks with optional filters
pub fn list_sqlite_tasks(
    epic_id: Option<&str>,
    status: Option<SqliteTaskStatus>,
    include_deleted: bool,
) -> Result<Vec<SqliteTask>, TasksError> {
    with_db(|conn| {
        let mut conditions = Vec::new();
        let mut param_values: Vec<String> = Vec::new();

        if let Some(eid) = epic_id {
            conditions.push(format!("epic_id = ?{}", param_values.len() + 1));
            param_values.push(eid.to_string());
        }

        if let Some(s) = status {
            conditions.push(format!("status = ?{}", param_values.len() + 1));
            param_values.push(s.as_str().to_string());
        }

        if !include_deleted {
            conditions.push("deleted_at IS NULL".to_string());
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT id, epic_id, title, description, priority, status, claimed_by, claimed_at,
                    lease_expires_at, heartbeat_at, created_at, updated_at, deleted_at
             FROM tasks_v2 {} ORDER BY priority, created_at",
            where_clause
        );

        let mut stmt = conn.prepare(&sql)?;

        let params_ref: Vec<&dyn rusqlite::ToSql> = param_values
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();

        let tasks = stmt
            .query_map(params_ref.as_slice(), |row| {
                let status_str: String = row.get(5)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    claimed_by: row.get(6)?,
                    claimed_at: row.get(7)?,
                    lease_expires_at: row.get(8)?,
                    heartbeat_at: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    deleted_at: row.get(12)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tasks)
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}

/// Get ready SQLite tasks (for display/debugging)
pub fn get_ready_sqlite_tasks(epic_id: Option<&str>) -> Result<Vec<SqliteTask>, TasksError> {
    with_db(|conn| {
        // Build query with optional epic filter using proper parameterization
        let has_epic_filter = epic_id.is_some();
        let epic_filter = if has_epic_filter { "AND t.epic_id = ?1" } else { "" };

        let sql = format!(r#"
            SELECT t.id, t.epic_id, t.title, t.description, t.priority, t.status,
                   t.claimed_by, t.claimed_at, t.lease_expires_at, t.heartbeat_at,
                   t.created_at, t.updated_at, t.deleted_at
            FROM tasks_v2 t
            WHERE t.status = 'open'
              AND t.deleted_at IS NULL
              {}
              -- All deps are closed OR deleted
              AND NOT EXISTS (
                  SELECT 1 FROM task_dependencies td
                  JOIN tasks_v2 dep ON dep.id = td.depends_on
                  WHERE td.task_id = t.id
                    AND dep.status != 'closed'
                    AND dep.deleted_at IS NULL
              )
              -- No footprint overlap with in_progress tasks
              AND NOT EXISTS (
                  SELECT 1 FROM task_footprints fp1
                  JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
                    AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
                  JOIN tasks_v2 other ON other.id = fp2.task_id
                  WHERE fp1.task_id = t.id
                    AND other.id != t.id
                    AND other.status = 'in_progress'
                    AND other.deleted_at IS NULL
              )
            ORDER BY t.priority, t.created_at
        "#, epic_filter);

        let mut stmt = conn.prepare(&sql)?;

        // Use different query paths based on whether we have an epic filter
        let tasks: Vec<SqliteTask> = if let Some(eid) = epic_id {
            stmt.query_map([eid], |row| {
                let status_str: String = row.get(5)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    claimed_by: row.get(6)?,
                    claimed_at: row.get(7)?,
                    lease_expires_at: row.get(8)?,
                    heartbeat_at: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    deleted_at: row.get(12)?,
                })
            })?.filter_map(|r| r.ok()).collect()
        } else {
            stmt.query_map([], |row| {
                let status_str: String = row.get(5)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    claimed_by: row.get(6)?,
                    claimed_at: row.get(7)?,
                    lease_expires_at: row.get(8)?,
                    heartbeat_at: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    deleted_at: row.get(12)?,
                })
            })?.filter_map(|r| r.ok()).collect()
        };

        Ok(tasks)
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
}

/// Soft-delete a SQLite task
pub fn soft_delete_sqlite_task(task_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        // First close the task if not already closed
        conn.execute(
            "UPDATE tasks_v2 SET status = 'closed', claimed_by = NULL, claimed_at = NULL,
             lease_expires_at = NULL, heartbeat_at = NULL, updated_at = ?1
             WHERE id = ?2 AND deleted_at IS NULL",
            params![now, task_id],
        )?;

        // Then set deleted_at (trigger enforces closed + unclaimed invariant)
        let affected = conn.execute(
            "UPDATE tasks_v2 SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![now, task_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task not found or already deleted: {}", task_id)),
            ));
        }

        Ok(())
    })
    .map_err(|e: rusqlite::Error| TasksError::DbError(e.to_string()))
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
        assert!(symbols_match("src/auth.rs::*", "src/auth.rs::login"));
        assert!(symbols_match("src/auth.rs::login", "src/auth.rs::*"));
        assert!(!symbols_match("src/auth.rs::*", "src/user.rs::login"));
        assert!(!symbols_match("src/auth.rs::login", "src/user.rs::*"));
    }

    #[test]
    fn test_symbols_match_bare_file_paths() {
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
        crate::db::init_db(Some(db_path.to_str().unwrap()), true).unwrap();
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
        }).unwrap();

        // Create task
        let input = CreateSqliteTaskInput {
            id: "TEST-001".to_string(),
            epic_id: "TEST-EPIC".to_string(),
            title: "Test Task".to_string(),
            description: Some("A test task".to_string()),
            priority: 3,
            depends_on: vec![],
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
        }).unwrap();

        create_sqlite_task(CreateSqliteTaskInput {
            id: "CLAIM-001".to_string(),
            epic_id: "CLAIM-EPIC".to_string(),
            title: "Claimable Task".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            footprint: TaskFootprint::default(),
        }).unwrap();

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
        }).unwrap();

        // Create tasks with different priorities
        create_sqlite_task(CreateSqliteTaskInput {
            id: "NEXT-LOW".to_string(),
            epic_id: "NEXT-EPIC".to_string(),
            title: "Low Priority".to_string(),
            description: None,
            priority: 10, // Lower priority (higher number)
            depends_on: vec![],
            footprint: TaskFootprint::default(),
        }).unwrap();

        create_sqlite_task(CreateSqliteTaskInput {
            id: "NEXT-HIGH".to_string(),
            epic_id: "NEXT-EPIC".to_string(),
            title: "High Priority".to_string(),
            description: None,
            priority: 1, // Higher priority (lower number)
            depends_on: vec![],
            footprint: TaskFootprint::default(),
        }).unwrap();

        // Should claim the higher priority task first
        let task = claim_next_sqlite_task("agent-1").unwrap();
        assert!(task.is_some());
        let task = task.unwrap();
        assert_eq!(task.id, "NEXT-HIGH");

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
        }).unwrap();

        // Create first task
        create_sqlite_task(CreateSqliteTaskInput {
            id: "DEP-001".to_string(),
            epic_id: "DEP-EPIC".to_string(),
            title: "First Task".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            footprint: TaskFootprint::default(),
        }).unwrap();

        // Create second task depending on first
        create_sqlite_task(CreateSqliteTaskInput {
            id: "DEP-002".to_string(),
            epic_id: "DEP-EPIC".to_string(),
            title: "Second Task".to_string(),
            description: None,
            priority: 5,
            depends_on: vec!["DEP-001".to_string()],
            footprint: TaskFootprint::default(),
        }).unwrap();

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
        }).unwrap();

        // Create tasks with overlapping footprints
        create_sqlite_task(CreateSqliteTaskInput {
            id: "FP-001".to_string(),
            epic_id: "FP-EPIC".to_string(),
            title: "First Modifier".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            footprint: TaskFootprint {
                modifies: vec!["src/auth.rs::Handler".to_string()],
                creates: vec![],
            },
        }).unwrap();

        create_sqlite_task(CreateSqliteTaskInput {
            id: "FP-002".to_string(),
            epic_id: "FP-EPIC".to_string(),
            title: "Second Modifier".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            footprint: TaskFootprint {
                modifies: vec!["src/auth.rs::Handler".to_string()], // Same symbol
                creates: vec![],
            },
        }).unwrap();

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

    #[test]
    fn test_heartbeat_sqlite_task() {
        let _dir = setup_test_db();

        // Setup
        crate::epics::create_epic(crate::epics::CreateEpicInput {
            id: "HB-EPIC".to_string(),
            title: "Heartbeat Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        }).unwrap();

        create_sqlite_task(CreateSqliteTaskInput {
            id: "HB-001".to_string(),
            epic_id: "HB-EPIC".to_string(),
            title: "Heartbeat Task".to_string(),
            description: None,
            priority: 5,
            depends_on: vec![],
            footprint: TaskFootprint::default(),
        }).unwrap();

        // Claim task
        let task = claim_sqlite_task("HB-001", "agent-1").unwrap();
        let original_heartbeat = task.heartbeat_at;

        // Small delay to ensure timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Send heartbeat
        heartbeat_sqlite_task("HB-001", "agent-1").unwrap();

        // Heartbeat from wrong agent should fail
        let result = heartbeat_sqlite_task("HB-001", "agent-2");
        assert!(result.is_err());

        crate::db::close_db();
    }
}
