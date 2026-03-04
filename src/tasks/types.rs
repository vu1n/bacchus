//! Type definitions for the task management module.

use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::collections::HashSet;
use std::str::FromStr;
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
    /// PM workflow type: bug_fix, feature, refactor, test, docs, infra, generic
    /// If not set, inferred from title/description
    #[serde(rename = "type", default)]
    pub task_type: Option<String>,
    /// Agent archetype: design, frontend, backend, data, test, infra, review, security, generic
    /// If not set, defaults to "generic"
    #[serde(default)]
    pub archetype: Option<String>,
    /// Task IDs that must be closed first
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Symbol-level footprint for collision detection
    #[serde(default)]
    pub footprint: TaskFootprint,
}

pub fn default_priority() -> i32 {
    5
}

pub fn default_status() -> String {
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

pub fn default_version() -> i32 {
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
#[cfg(test)]
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

impl From<rusqlite::Error> for TasksError {
    fn from(e: rusqlite::Error) -> Self {
        TasksError::DbError(e.to_string())
    }
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
    ReadyForRelease, // Agent marked ready, awaiting orchestrator release
    Releasing,       // Orchestrator is attempting release (rebase/merge)
    NeedsResolution, // Release failed due to conflicts, needs human/agent resolution
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
}

impl FromStr for SqliteTaskStatus {
    type Err = TasksError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
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

/// Task type for PM workflow categorization
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

    pub fn from_str_lossy(s: &str) -> Self {
        s.parse().unwrap_or(SqliteTaskType::Generic)
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

impl FromStr for SqliteTaskType {
    type Err = TasksError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bug_fix" => Ok(SqliteTaskType::BugFix),
            "feature" => Ok(SqliteTaskType::Feature),
            "refactor" => Ok(SqliteTaskType::Refactor),
            "test" => Ok(SqliteTaskType::Test),
            "docs" => Ok(SqliteTaskType::Docs),
            "infra" => Ok(SqliteTaskType::Infra),
            "generic" => Ok(SqliteTaskType::Generic),
            _ => Err(TasksError::InvalidStatus(format!(
                "Unknown task type: {}",
                s
            ))),
        }
    }
}

impl std::fmt::Display for SqliteTaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Infer task type from title and description
///
/// Infers PM workflow type (bug_fix, feature, refactor, test, docs, infra, generic)
/// based on keywords in the title/description.
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
    /// PM workflow type (bug_fix, feature, refactor, test, docs, infra, generic)
    pub task_type: SqliteTaskType,
    /// Agent archetype (design, frontend, backend, data, test, infra, review, security, generic)
    pub archetype: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<i64>,
    /// Last claim heartbeat timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_heartbeat_at: Option<i64>,
    /// jj commit ID when agent marks task ready (pre-rebase)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_commit_id: Option<String>,
    /// jj commit ID after orchestrator rebases onto main (for stuck detection)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_commit_id: Option<String>,
    /// When orchestrator started release attempt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_started_at: Option<i64>,
    /// When task was closed (completed_at timestamp)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorLease {
    pub lease_name: String,
    pub holder_id: String,
    pub lease_expires_at: i64,
    pub updated_at: i64,
}

/// Input for creating a new SQLite task
#[derive(Debug, Clone)]
pub struct CreateSqliteTaskInput {
    pub id: String,
    pub epic_id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: i32,
    /// Optional PM workflow type (inferred from title if not provided)
    pub task_type: Option<SqliteTaskType>,
    /// Optional archetype (defaults to "generic" if not provided)
    pub archetype: Option<String>,
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

/// Validation result for a single task
#[derive(Debug, Serialize, Deserialize)]
pub struct TaskValidation {
    pub task_id: String,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}
