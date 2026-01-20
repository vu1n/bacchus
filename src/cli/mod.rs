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

    /// Review a task before release (advisory checks)
    Review {
        /// The task ID to review
        task_id: String,
        /// Build command to run (optional)
        #[arg(long)]
        build_cmd: Option<String>,
        /// Test command to run (optional)
        #[arg(long)]
        test_cmd: Option<String>,
    },

    /// Generate eval metrics report
    Eval {
        /// Filter by epic ID
        #[arg(long)]
        epic: Option<String>,
        /// Number of days to include (default: 7)
        #[arg(long, default_value = "7")]
        days: i64,
    },

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

    /// Manage epics (high-level work containers)
    Epic {
        #[command(subcommand)]
        command: EpicCommands,
    },

    /// Manage agent messages (for debugging/monitoring)
    Message {
        #[command(subcommand)]
        command: MessageCommands,
    },
}

#[derive(Subcommand)]
pub enum SessionCommands {
    /// Start a session (agent, orchestrator, or architect mode)
    Start {
        /// Mode: agent, orchestrator, or architect
        mode: String,
        /// Task ID (required for agent mode)
        #[arg(long)]
        task_id: Option<String>,
        /// Max concurrent agents (for orchestrator mode)
        #[arg(long, default_value = "3")]
        max_concurrent: i32,
        /// Agent ID (required for architect mode)
        #[arg(long)]
        agent_id: Option<String>,
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

    /// Validate tasks against the symbol index
    Validate,

    /// Initialize a tasks.yaml template
    Init,

    /// Import tasks from YAML to SQLite
    Import {
        /// Epic ID to import tasks into (auto-generated if not specified)
        #[arg(long)]
        epic_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum EpicCommands {
    /// List epics with optional status filter
    List {
        /// Filter by status (open, planning, active, closed)
        #[arg(long)]
        status: Option<String>,
    },

    /// Show details for a specific epic (with task counts)
    Show {
        /// The epic ID to show
        id: String,
    },

    /// Create a new epic
    Create {
        /// Epic ID (e.g., AUTH-EPIC)
        #[arg(long)]
        id: String,
        /// Epic title
        #[arg(long)]
        title: String,
        /// Epic description
        #[arg(long)]
        description: Option<String>,
    },

    /// Assign an epic to an architect agent for breakdown
    Assign {
        /// The epic ID to assign
        id: String,
        /// The architect agent ID to assign to
        agent: String,
    },
}

#[derive(Subcommand)]
pub enum MessageCommands {
    /// List messages with optional filters
    List {
        /// Filter by target agent
        #[arg(long)]
        agent: Option<String>,
        /// Filter by status (pending, processing, processed, failed)
        #[arg(long)]
        status: Option<String>,
    },

    /// Send a message to an agent (for testing/debugging)
    Send {
        /// Target agent ID
        agent: String,
        /// Message type (e.g., epic_assigned)
        message_type: String,
        /// JSON payload
        payload: String,
    },
}
