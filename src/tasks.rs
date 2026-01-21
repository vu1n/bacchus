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

use crate::db::with_db;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

    #[error("No tasks file found: {0}")]
    NoTasksFile(String),
}

// ============================================================================
// Import Support
// ============================================================================

/// Result of importing YAML tasks to SQLite
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    /// Number of tasks imported
    pub imported: usize,
    /// Number of tasks skipped (already exist)
    pub skipped: usize,
    /// Task IDs that were imported
    pub imported_ids: Vec<String>,
    /// Task IDs that were skipped
    pub skipped_ids: Vec<String>,
    /// Epic ID used for imported tasks
    pub epic_id: String,
    /// Any warnings generated during import
    pub warnings: Vec<String>,
}

// ============================================================================
// SQLite Task Types (tasks)
// ============================================================================

/// Task status for SQLite tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteTaskStatus {
    Draft,
    Open,
    InProgress,
    ReadyForRelease,  // Agent marked ready, awaiting orchestrator release
    Releasing,        // Orchestrator is attempting release (rebase/merge)
    NeedsResolution,  // Release failed due to conflicts, needs human/agent resolution
    Blocked,
    Closed,
}

impl SqliteTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SqliteTaskStatus::Draft => "draft",
            SqliteTaskStatus::Open => "open",
            SqliteTaskStatus::InProgress => "in_progress",
            SqliteTaskStatus::ReadyForRelease => "ready_for_release",
            SqliteTaskStatus::Releasing => "releasing",
            SqliteTaskStatus::NeedsResolution => "needs_resolution",
            SqliteTaskStatus::Blocked => "blocked",
            SqliteTaskStatus::Closed => "closed",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, TasksError> {
        match s {
            "draft" => Ok(SqliteTaskStatus::Draft),
            "open" => Ok(SqliteTaskStatus::Open),
            "in_progress" => Ok(SqliteTaskStatus::InProgress),
            "ready_for_release" => Ok(SqliteTaskStatus::ReadyForRelease),
            "releasing" => Ok(SqliteTaskStatus::Releasing),
            "needs_resolution" => Ok(SqliteTaskStatus::NeedsResolution),
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

/// Task type for context-aware prompting
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SqliteTaskType {
    BugFix,
    Feature,
    Refactor,
    Test,
    Docs,
    Infra,
    Generic,
}

impl SqliteTaskType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SqliteTaskType::BugFix => "bug_fix",
            SqliteTaskType::Feature => "feature",
            SqliteTaskType::Refactor => "refactor",
            SqliteTaskType::Test => "test",
            SqliteTaskType::Docs => "docs",
            SqliteTaskType::Infra => "infra",
            SqliteTaskType::Generic => "generic",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "bug_fix" => SqliteTaskType::BugFix,
            "feature" => SqliteTaskType::Feature,
            "refactor" => SqliteTaskType::Refactor,
            "test" => SqliteTaskType::Test,
            "docs" => SqliteTaskType::Docs,
            "infra" => SqliteTaskType::Infra,
            _ => SqliteTaskType::Generic,
        }
    }

    /// Human-readable label for display
    pub fn label(&self) -> &'static str {
        match self {
            SqliteTaskType::BugFix => "Bug Fix",
            SqliteTaskType::Feature => "Feature",
            SqliteTaskType::Refactor => "Refactor",
            SqliteTaskType::Test => "Test",
            SqliteTaskType::Docs => "Documentation",
            SqliteTaskType::Infra => "Infrastructure",
            SqliteTaskType::Generic => "Generic",
        }
    }
}

impl std::fmt::Display for SqliteTaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Infer task type from title and description
pub fn infer_task_type(title: &str, description: Option<&str>) -> SqliteTaskType {
    let text = format!(
        "{} {}",
        title.to_lowercase(),
        description.unwrap_or("").to_lowercase()
    );

    // Bug fix patterns
    if text.contains("fix")
        || text.contains("bug")
        || text.contains("error")
        || text.contains("crash")
        || text.contains("issue")
        || text.contains("broken")
        || text.contains("incorrect")
        || text.contains("wrong")
    {
        return SqliteTaskType::BugFix;
    }

    // Feature patterns
    if text.contains("add")
        || text.contains("implement")
        || text.contains("create")
        || text.contains("new")
        || text.contains("feature")
        || text.contains("support")
        || text.contains("enable")
    {
        return SqliteTaskType::Feature;
    }

    // Refactor patterns
    if text.contains("refactor")
        || text.contains("cleanup")
        || text.contains("clean up")
        || text.contains("reorganize")
        || text.contains("restructure")
        || text.contains("simplify")
        || text.contains("improve")
        || text.contains("optimize")
    {
        return SqliteTaskType::Refactor;
    }

    // Test patterns
    if text.contains("test")
        || text.contains("spec")
        || text.contains("coverage")
        || text.contains("mock")
        || text.contains("stub")
    {
        return SqliteTaskType::Test;
    }

    // Docs patterns
    if text.contains("doc")
        || text.contains("readme")
        || text.contains("comment")
        || text.contains("explain")
        || text.contains("describe")
    {
        return SqliteTaskType::Docs;
    }

    // Infra patterns
    if text.contains("ci")
        || text.contains("cd")
        || text.contains("deploy")
        || text.contains("build")
        || text.contains("config")
        || text.contains("docker")
        || text.contains("kubernetes")
        || text.contains("k8s")
        || text.contains("terraform")
        || text.contains("infra")
        || text.contains("pipeline")
    {
        return SqliteTaskType::Infra;
    }

    SqliteTaskType::Generic
}

/// A task stored in SQLite (tasks table)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqliteTask {
    pub id: String,
    pub epic_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub priority: i32,
    pub status: SqliteTaskStatus,
    pub task_type: SqliteTaskType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<i64>,
    /// jj commit ID when agent marks task ready (pre-rebase)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_commit_id: Option<String>,
    /// jj commit ID after orchestrator rebases onto main (for stuck detection)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_commit_id: Option<String>,
    /// When orchestrator started release attempt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_started_at: Option<i64>,
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

/// Load tasks from the YAML file (read-only for import purposes)
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
#[allow(dead_code)]
fn get_active_footprints() -> Result<ResolvedFootprint, TasksError> {
    with_db(|conn| {
        let mut resolved = ResolvedFootprint::default();

        let mut stmt = conn.prepare(
            "SELECT pattern_type, file_path, symbol, is_wildcard
             FROM task_footprints tf
             JOIN tasks t ON t.id = tf.task_id
             WHERE t.status = 'in_progress' AND t.deleted_at IS NULL",
        )?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                ))
            })?;

        for row_result in rows {
            if let Ok((pattern_type, file_path, symbol, is_wildcard)) = row_result {
                match pattern_type.as_str() {
                    "modifies" => {
                        let pattern = if is_wildcard == 1 {
                            format!("{}::*", file_path)
                        } else {
                            format!("{}::{}", file_path, symbol)
                        };
                        resolved.symbols.insert(pattern);
                    }
                    "creates" => {
                        resolved.creates.insert(file_path);
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
// Validation
// ============================================================================

/// Validation result for a single task
#[derive(Debug, Serialize, Deserialize)]
pub struct TaskValidation {
    pub task_id: String,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

/// Validate tasks using SQLite dependencies and footprints
pub fn validate_tasks(_workspace_root: &Path) -> Result<Vec<TaskValidation>, TasksError> {
    let tasks = list_sqlite_tasks(None, None, false)?;
    let mut validations: HashMap<String, TaskValidation> = tasks
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                TaskValidation {
                    task_id: t.id.clone(),
                    warnings: Vec::new(),
                    errors: Vec::new(),
                },
            )
        })
        .collect();

    // Validate footprint syntax based on normalized entries
    let footprint_rows: Vec<(String, String, String, String, i32)> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT task_id, pattern_type, file_path, symbol, is_wildcard
             FROM task_footprints",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i32>(4)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))?;

    for (task_id, pattern_type, file_path, symbol, is_wildcard) in footprint_rows {
        let Some(validation) = validations.get_mut(&task_id) else {
            continue;
        };

        if file_path.trim().is_empty() {
            validation
                .errors
                .push("Footprint has empty file path".to_string());
        }

        if is_wildcard == 0 && symbol.trim().is_empty() {
            validation
                .errors
                .push("Footprint symbol is empty without wildcard".to_string());
        }

        if is_wildcard == 1 && !symbol.is_empty() {
            validation
                .errors
                .push("Footprint wildcard has unexpected symbol".to_string());
        }

        if pattern_type != "modifies" && pattern_type != "creates" {
            validation.errors.push(format!(
                "Footprint has invalid pattern type: {}",
                pattern_type
            ));
        }
    }

    // Detect dependency cycles
    let deps: Vec<(String, String)> = with_db(|conn| {
        let mut stmt = conn.prepare("SELECT task_id, depends_on FROM task_dependencies")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))?;

    let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
    for (task_id, depends_on) in deps {
        deps_map.entry(task_id).or_default().push(depends_on);
    }

    let cycle_tasks = detect_dependency_cycles(&tasks, &deps_map);
    for task_id in cycle_tasks {
        if let Some(validation) = validations.get_mut(&task_id) {
            validation.errors.push("Dependency cycle detected".to_string());
        }
    }

    // Check footprint overlaps between open tasks
    let overlaps: Vec<(String, String, String)> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t1.id, t2.id, fp1.file_path
             FROM task_footprints fp1
             JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
               AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
             JOIN tasks t1 ON t1.id = fp1.task_id
             JOIN tasks t2 ON t2.id = fp2.task_id
             WHERE t1.id < t2.id
               AND t1.status = 'open'
               AND t2.status = 'open'
               AND t1.deleted_at IS NULL
               AND t2.deleted_at IS NULL",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))?;

    for (task_a, task_b, file_path) in overlaps {
        if let Some(validation) = validations.get_mut(&task_a) {
            validation
                .warnings
                .push(format!("Footprint overlaps with {} on {}", task_b, file_path));
        }
        if let Some(validation) = validations.get_mut(&task_b) {
            validation
                .warnings
                .push(format!("Footprint overlaps with {} on {}", task_a, file_path));
        }
    }

    let mut ordered = Vec::new();
    for task in tasks {
        if let Some(validation) = validations.remove(&task.id) {
            ordered.push(validation);
        }
    }

    Ok(ordered)
}

fn detect_dependency_cycles(
    tasks: &[SqliteTask],
    deps: &HashMap<String, Vec<String>>,
) -> HashSet<String> {
    let mut visiting: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    let mut in_cycle: HashSet<String> = HashSet::new();

    fn dfs(
        node: &str,
        deps: &HashMap<String, Vec<String>>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
        stack: &mut Vec<String>,
        in_cycle: &mut HashSet<String>,
    ) {
        if visiting.contains(node) {
            if let Some(pos) = stack.iter().position(|n| n == node) {
                for id in &stack[pos..] {
                    in_cycle.insert(id.clone());
                }
            }
            return;
        }
        if visited.contains(node) {
            return;
        }

        visiting.insert(node.to_string());
        stack.push(node.to_string());

        if let Some(next) = deps.get(node) {
            for dep in next {
                dfs(dep, deps, visiting, visited, stack, in_cycle);
            }
        }

        stack.pop();
        visiting.remove(node);
        visited.insert(node.to_string());
    }

    for task in tasks {
        dfs(&task.id, deps, &mut visiting, &mut visited, &mut stack, &mut in_cycle);
    }

    in_cycle
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
// SQLite Task Operations (tasks)
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
            "SELECT 1 FROM tasks WHERE id = ?1",
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

        // Infer task type from title/description
        let task_type = infer_task_type(&input.title, input.description.as_deref());

        let result = (|| -> rusqlite::Result<(i64, SqliteTaskType)> {
            // Insert task as draft
            conn.execute(
                "INSERT INTO tasks (id, epic_id, title, description, priority, status, task_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'draft', ?6, ?7, ?7)",
                params![input.id, input.epic_id, input.title, input.description, input.priority, task_type.as_str(), now],
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
                "UPDATE tasks SET status = 'open', updated_at = ?2 WHERE id = ?1",
                params![input.id, flip_time],
            )?;

            Ok((flip_time, task_type))
        })();

        match result {
            Ok((flip_time, task_type)) => {
                conn.execute("RELEASE create_task", [])?;
                Ok(SqliteTask {
                    id: input.id,
                    epic_id: input.epic_id,
                    title: input.title,
                    description: input.description,
                    priority: input.priority,
                    status: SqliteTaskStatus::Open,
                    task_type,
                    claimed_by: None,
                    claimed_at: None,
                    ready_commit_id: None,
                    release_commit_id: None,
                    release_started_at: None,
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

    with_db(|conn| {
        // Atomic claim with embedded readiness check
        // This is a complex query but it runs as a single atomic UPDATE
        conn.execute(
            r#"
            UPDATE tasks
            SET status = 'in_progress',
                claimed_by = ?1,
                claimed_at = ?2,
                updated_at = ?2
            WHERE id = (
                SELECT t.id FROM tasks t
                WHERE t.status = 'open'
                  AND t.deleted_at IS NULL
                  -- All deps are closed OR deleted
                  AND NOT EXISTS (
                      SELECT 1 FROM task_dependencies td
                      JOIN tasks dep ON dep.id = td.depends_on
                      WHERE td.task_id = t.id
                        AND dep.status != 'closed'
                        AND dep.deleted_at IS NULL
                  )
                  -- No footprint overlap with OTHER in_progress tasks
                  AND NOT EXISTS (
                      SELECT 1 FROM task_footprints fp1
                      JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
                        AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
                      JOIN tasks other ON other.id = fp2.task_id
                      WHERE fp1.task_id = t.id
                        AND other.id != t.id
                        AND other.status = 'in_progress'
                        AND other.deleted_at IS NULL
                  )
                ORDER BY t.priority, t.created_at
                LIMIT 1
            )
            "#,
            params![agent_id, now],
        )?;

        // Check if we claimed anything
        let changes = conn.changes();
        if changes == 0 {
            return Ok(None);
        }

        // Fetch the claimed task
        let task = conn.query_row(
            "SELECT id, epic_id, title, description, priority, status, task_type, claimed_by, claimed_at,
                    ready_commit_id, release_commit_id, release_started_at, created_at, updated_at, deleted_at
             FROM tasks WHERE claimed_by = ?1 AND status = 'in_progress'
             ORDER BY claimed_at DESC LIMIT 1",
            [agent_id],
            |row| {
                let status_str: String = row.get(5)?;
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
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

    with_db(|conn| {
        // Atomic claim with readiness check
        let affected = conn.execute(
            r#"
            UPDATE tasks
            SET status = 'in_progress',
                claimed_by = ?1,
                claimed_at = ?2,
                updated_at = ?2
            WHERE id = ?3
              AND status = 'open'
              AND deleted_at IS NULL
              -- All deps are closed OR deleted
              AND NOT EXISTS (
                  SELECT 1 FROM task_dependencies td
                  JOIN tasks dep ON dep.id = td.depends_on
                  WHERE td.task_id = ?3
                    AND dep.status != 'closed'
                    AND dep.deleted_at IS NULL
              )
              -- No footprint overlap with OTHER in_progress tasks
              AND NOT EXISTS (
                  SELECT 1 FROM task_footprints fp1
                  JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
                    AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
                  JOIN tasks other ON other.id = fp2.task_id
                  WHERE fp1.task_id = ?3
                    AND other.id != ?3
                    AND other.status = 'in_progress'
                    AND other.deleted_at IS NULL
              )
            "#,
            params![agent_id, now, task_id],
        )?;

        if affected == 0 {
            // Check why claim failed
            let task_status: Option<String> = conn.query_row(
                "SELECT status FROM tasks WHERE id = ?1 AND deleted_at IS NULL",
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
            "SELECT id, epic_id, title, description, priority, status, task_type, claimed_by, claimed_at,
                    ready_commit_id, release_commit_id, release_started_at, created_at, updated_at, deleted_at
             FROM tasks WHERE id = ?1",
            [task_id],
            |row| {
                let status_str: String = row.get(5)?;
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
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

/// Release a SQLite task (mark as closed, clear claim)
pub fn release_sqlite_task(task_id: &str, agent_id: &str) -> Result<SqliteTask, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks
             SET status = 'closed',
                 claimed_by = NULL,
                 claimed_at = NULL,
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
            "SELECT id, epic_id, title, description, priority, status, task_type, claimed_by, claimed_at,
                    ready_commit_id, release_commit_id, release_started_at, created_at, updated_at, deleted_at
             FROM tasks WHERE id = ?1",
            [task_id],
            |row| {
                let status_str: String = row.get(5)?;
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Closed),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
                })
            },
        )
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
            "SELECT id, epic_id, title, description, priority, status, task_type, claimed_by, claimed_at,
                    ready_commit_id, release_commit_id, release_started_at, created_at, updated_at, deleted_at
             FROM tasks {} ORDER BY priority, created_at",
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
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
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
            SELECT t.id, t.epic_id, t.title, t.description, t.priority, t.status, t.task_type,
                   t.claimed_by, t.claimed_at, t.ready_commit_id, t.release_commit_id, t.release_started_at,
                   t.created_at, t.updated_at, t.deleted_at
            FROM tasks t
            WHERE t.status = 'open'
              AND t.deleted_at IS NULL
              {}
              -- All deps are closed OR deleted
              AND NOT EXISTS (
                  SELECT 1 FROM task_dependencies td
                  JOIN tasks dep ON dep.id = td.depends_on
                  WHERE td.task_id = t.id
                    AND dep.status != 'closed'
                    AND dep.deleted_at IS NULL
              )
              -- No footprint overlap with in_progress tasks
              AND NOT EXISTS (
                  SELECT 1 FROM task_footprints fp1
                  JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
                    AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
                  JOIN tasks other ON other.id = fp2.task_id
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
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
                })
            })?.filter_map(|r| r.ok()).collect()
        } else {
            stmt.query_map([], |row| {
                let status_str: String = row.get(5)?;
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
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
            "UPDATE tasks SET status = 'closed', claimed_by = NULL, claimed_at = NULL,
             updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![now, task_id],
        )?;

        // Then set deleted_at (trigger enforces closed + unclaimed invariant)
        let affected = conn.execute(
            "UPDATE tasks SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
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
// SQLite Task Operations
// ============================================================================

/// Get a SQLite task by ID
pub fn get_sqlite_task(task_id: &str) -> Result<SqliteTask, TasksError> {
    with_db(|conn| {
        conn.query_row(
            "SELECT id, epic_id, title, description, priority, status, task_type, claimed_by, claimed_at,
                    ready_commit_id, release_commit_id, release_started_at, created_at, updated_at, deleted_at
             FROM tasks WHERE id = ?1 AND deleted_at IS NULL",
            [task_id],
            |row| {
                let status_str: String = row.get(5)?;
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str).unwrap_or(SqliteTaskStatus::Open),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
                })
            },
        )
    })
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => TasksError::TaskNotFound(task_id.to_string()),
        e => TasksError::DbError(e.to_string()),
    })
}

/// Update a SQLite task's status
pub fn update_sqlite_task_status(task_id: &str, status: SqliteTaskStatus) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3 AND deleted_at IS NULL",
            params![status.as_str(), now, task_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task not found: {}", task_id)),
            ));
        }

        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Reset a task to a status and clear claim metadata
pub fn reset_sqlite_task(task_id: &str, status: SqliteTaskStatus) -> Result<(), TasksError> {
    match status {
        SqliteTaskStatus::Open | SqliteTaskStatus::Blocked => {}
        _ => {
            return Err(TasksError::InvalidStatus(status.as_str().to_string()));
        }
    }

    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks
             SET status = ?1,
                 claimed_by = NULL,
                 claimed_at = NULL,
                 updated_at = ?2
             WHERE id = ?3 AND deleted_at IS NULL",
            params![status.as_str(), now, task_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task not found: {}", task_id)),
            ));
        }

        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

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
pub fn import_yaml_tasks(workspace_root: &Path, epic_id: Option<&str>) -> Result<ImportResult, TasksError> {
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
                .query_row(
                    "SELECT 1 FROM tasks WHERE id = ?1",
                    [&task.id],
                    |_| Ok(true),
                )
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
        let input = CreateSqliteTaskInput {
            id: task.id.clone(),
            epic_id: epic_id.clone(),
            title: task.title.clone(),
            description: task.description.clone(),
            priority: task.priority,
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
fn ensure_epic_exists(epic_id: &str) -> Result<(), TasksError> {
    let exists = with_db(|conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM epics WHERE id = ?1",
                [epic_id],
                |_| Ok(true),
            )
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

// ============================================================================
// jj Workspace Release Workflow
// ============================================================================

/// Mark a task as ready for release (agent calls this when work is complete)
///
/// Stores the pre-rebase commit ID and transitions to ready_for_release status.
/// The orchestrator will later attempt to release this task.
pub fn mark_task_ready_for_release(
    task_id: &str,
    agent_id: &str,
    commit_id: &str,
) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks
             SET status = 'ready_for_release',
                 ready_commit_id = ?1,
                 updated_at = ?2
             WHERE id = ?3
               AND claimed_by = ?4
               AND status = 'in_progress'
               AND deleted_at IS NULL",
            params![commit_id, now, task_id, agent_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!(
                    "Task {} not owned by {} or not in_progress",
                    task_id, agent_id
                )),
            ));
        }

        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Start releasing a task (orchestrator calls this before attempting rebase)
///
/// Transitions to releasing status and records the release start time.
/// Uses BEGIN IMMEDIATE for atomic locking.
pub fn start_task_release(task_id: &str, release_commit_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks
             SET status = 'releasing',
                 release_commit_id = ?1,
                 release_started_at = ?2,
                 updated_at = ?2
             WHERE id = ?3
               AND status = 'ready_for_release'
               AND deleted_at IS NULL",
            params![release_commit_id, now, task_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task {} not ready_for_release", task_id)),
            ));
        }

        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Complete task release successfully (orchestrator calls after advancing main)
///
/// Clears claim metadata and transitions to closed.
pub fn complete_task_release(task_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks
             SET status = 'closed',
                 claimed_by = NULL,
                 claimed_at = NULL,
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'releasing'
               AND deleted_at IS NULL",
            params![now, task_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task {} not in releasing status", task_id)),
            ));
        }

        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Mark task as needing resolution (orchestrator calls when conflicts occur)
///
/// Transitions to needs_resolution status. Agent or human must resolve conflicts.
pub fn mark_task_needs_resolution(task_id: &str, conflict_files: &[String]) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks
             SET status = 'needs_resolution',
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'releasing'
               AND deleted_at IS NULL",
            params![now, task_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task {} not in releasing status", task_id)),
            ));
        }

        // Log conflict files for debugging (could be stored in a separate table)
        if !conflict_files.is_empty() {
            eprintln!(
                "Task {} has conflicts in: {}",
                task_id,
                conflict_files.join(", ")
            );
        }

        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Get tasks ready for release (orchestrator uses this to find work)
pub fn get_tasks_ready_for_release() -> Result<Vec<SqliteTask>, TasksError> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, epic_id, title, description, priority, status, task_type, claimed_by, claimed_at,
                    ready_commit_id, release_commit_id, release_started_at, created_at, updated_at, deleted_at
             FROM tasks
             WHERE status = 'ready_for_release'
               AND deleted_at IS NULL
             ORDER BY priority, created_at",
        )?;

        let tasks = stmt
            .query_map([], |row| {
                let status_str: String = row.get(5)?;
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str)
                        .unwrap_or(SqliteTaskStatus::ReadyForRelease),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tasks)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Get tasks needing resolution (for monitoring/alerting)
pub fn get_tasks_needing_resolution() -> Result<Vec<SqliteTask>, TasksError> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, epic_id, title, description, priority, status, task_type, claimed_by, claimed_at,
                    ready_commit_id, release_commit_id, release_started_at, created_at, updated_at, deleted_at
             FROM tasks
             WHERE status = 'needs_resolution'
               AND deleted_at IS NULL
             ORDER BY priority, created_at",
        )?;

        let tasks = stmt
            .query_map([], |row| {
                let status_str: String = row.get(5)?;
                let task_type_str: String = row.get(6)?;
                Ok(SqliteTask {
                    id: row.get(0)?,
                    epic_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    priority: row.get(4)?,
                    status: SqliteTaskStatus::from_str(&status_str)
                        .unwrap_or(SqliteTaskStatus::NeedsResolution),
                    task_type: SqliteTaskType::from_str(&task_type_str),
                    claimed_by: row.get(7)?,
                    claimed_at: row.get(8)?,
                    ready_commit_id: row.get(9)?,
                    release_commit_id: row.get(10)?,
                    release_started_at: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    deleted_at: row.get(14)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tasks)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Reset task from needs_resolution back to in_progress (after resolving conflicts)
pub fn reset_task_from_resolution(task_id: &str) -> Result<(), TasksError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE tasks
             SET status = 'in_progress',
                 release_commit_id = NULL,
                 release_started_at = NULL,
                 updated_at = ?1
             WHERE id = ?2
               AND status = 'needs_resolution'
               AND deleted_at IS NULL",
            params![now, task_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!("Task {} not in needs_resolution status", task_id)),
            ));
        }

        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
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
