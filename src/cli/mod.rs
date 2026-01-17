//! CLI module for Bacchus
//!
//! Defines command-line interface using clap.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bacchus")]
#[command(about = "Worktree-based coordination CLI for multi-agent work")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    // ========================================================================
    // Coordination Commands (worktree-based)
    // ========================================================================

    /// Get next ready task, create worktree, claim it
    Next {
        /// Your agent ID
        agent_id: String,
    },

    /// Claim a specific task by ID, create worktree
    Claim {
        /// The task ID to claim
        task_id: String,
        /// Your agent ID
        agent_id: String,
        /// Force claim even if task is not ready (blocked/in_progress)
        #[arg(long)]
        force: bool,
    },

    /// Release a claimed task
    Release {
        /// The task ID to release
        task_id: String,
        /// Release status: done (merge), blocked (keep), or failed (discard)
        #[arg(long, default_value = "done")]
        status: String,
    },

    /// Abort a failed merge for a task
    Abort {
        /// The task ID with a failed merge
        task_id: String,
    },

    /// Resolve a merge conflict after manual resolution
    Resolve {
        /// The task ID with resolved conflicts
        task_id: String,
    },

    /// Find stale claims and optionally clean them up
    Stale {
        /// Minutes without activity to consider stale
        #[arg(short, long, default_value = "15")]
        minutes: i64,
        /// Clean up stale claims (remove worktrees, reset tasks)
        #[arg(long)]
        cleanup: bool,
    },

    /// List all active claims and worktrees
    List,

    // ========================================================================
    // Symbol Commands
    // ========================================================================

    /// Search for symbols in the codebase
    Symbols {
        /// Name pattern (supports * wildcards)
        #[arg(short, long)]
        pattern: Option<String>,
        /// Filter by kind (function, class, method, interface, type, variable)
        #[arg(short, long)]
        kind: Option<String>,
        /// Filter by file path (supports * wildcards)
        #[arg(short, long)]
        file: Option<String>,
        /// Filter by language (typescript, javascript, python, go, rust)
        #[arg(short, long)]
        lang: Option<String>,
        /// Max results
        #[arg(short = 'n', long, default_value = "50")]
        limit: i32,
        /// Full-text search query (searches name + docstring)
        #[arg(long)]
        search: Option<String>,
        /// Enable fuzzy matching for typo tolerance
        #[arg(long)]
        fuzzy: bool,
    },

    /// Index a file or directory for symbol search
    Index {
        /// Path to file or directory to index
        path: String,
    },

    // ========================================================================
    // Info Commands
    // ========================================================================

    /// Show current claims and status
    Status,

    /// Print workflow documentation
    Workflow,

    /// Generate context for the current agent (global or task-specific)
    Context {
        /// Force context for a specific task ID
        #[arg(long)]
        task_id: Option<String>,
    },

    /// Update bacchus to the latest version
    SelfUpdate,

    /// Check if a newer version is available
    CheckUpdate,

    /// Manage session state for stop hooks
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// Manage tasks (built-in task management)
    Task {
        #[command(subcommand)]
        command: TaskCommands,
    },
}

#[derive(Subcommand)]
pub enum SessionCommands {
    /// Start a session (agent or orchestrator mode)
    Start {
        /// Mode: agent or orchestrator
        mode: String,
        /// Task ID (required for agent mode)
        #[arg(long)]
        task_id: Option<String>,
        /// Max concurrent agents (for orchestrator mode)
        #[arg(long, default_value = "3")]
        max_concurrent: i32,
    },

    /// Stop the current session
    Stop,

    /// Show current session status
    Status,

    /// Check if session should block exit (for stop hook)
    Check,
}

#[derive(Subcommand)]
pub enum TaskCommands {
    /// List tasks with optional filters
    List {
        /// Filter by status (open, in_progress, blocked, closed)
        #[arg(long)]
        status: Option<String>,
        /// Show only ready tasks (open with satisfied dependencies)
        #[arg(long)]
        ready: bool,
    },

    /// Show details for a specific task
    Show {
        /// The task ID to show
        id: String,
    },

    /// Add a new task
    Add {
        /// Task ID (e.g., AUTH-001)
        #[arg(long)]
        id: String,
        /// Task title
        #[arg(long)]
        title: String,
        /// Task description
        #[arg(long)]
        description: Option<String>,
        /// Priority (lower = higher priority, default: 5)
        #[arg(long)]
        priority: Option<i32>,
        /// Comma-separated list of task IDs this depends on
        #[arg(long)]
        deps: Option<String>,
    },

    /// Validate tasks against the symbol index
    Validate,

    /// Initialize a tasks.yaml template
    Init,
}
