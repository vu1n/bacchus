use crate::tools::{ReleaseStatus, SessionMode};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bacchus", about = "Workspace-based coordination CLI for multi-agent work", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Find next ready task, create workspace, and claim it
    Next { agent_id: String },

    /// Claim a specific task by ID
    Claim {
        task_id: String,
        agent_id: String,
        #[arg(long, help = "Force claim even if not ready")]
        force: bool,
    },

    /// Release a claimed task
    Release {
        task_id: String,
        #[arg(long, value_enum, default_value = "done")]
        status: ReleaseStatus,
    },

    /// Abort a release that needs resolution
    Abort { task_id: String },

    /// Re-release after resolving merge conflicts
    Resolve { task_id: String },

    /// Find stale claims and optionally clean them up
    Stale {
        #[arg(short, long, default_value = "15")]
        minutes: i64,
        #[arg(long)]
        cleanup: bool,
    },

    /// List active claims and workspaces
    List,

    /// Record heartbeat for an active claim
    Heartbeat { task_id: String, agent_id: String },

    /// Advisory pre-release checks (build, test, footprint)
    Review {
        task_id: String,
        #[arg(long)]
        build_cmd: Option<String>,
        #[arg(long)]
        test_cmd: Option<String>,
    },

    /// Merge tasks that are ready_for_release
    ProcessReleases {
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// Generate eval metrics report
    Eval {
        #[arg(long)]
        epic: Option<String>,
        #[arg(long, default_value = "7")]
        days: i64,
    },

    /// Bootstrap bacchus in this repository
    Init {
        #[arg(long)]
        skip_jj: bool,
        #[arg(long)]
        force_tasks: bool,
        #[arg(long)]
        epic_id: Option<String>,
        #[arg(long, requires = "epic_id")]
        epic_title: Option<String>,
    },

    /// Search indexed symbols
    Symbols {
        #[arg(short, long)]
        pattern: Option<String>,
        #[arg(short, long, help = "Filter by kind: function, class, method, interface, type, variable")]
        kind: Option<String>,
        #[arg(short, long)]
        file: Option<String>,
        #[arg(short, long)]
        lang: Option<String>,
        #[arg(short = 'n', long, default_value = "50")]
        limit: i32,
        #[arg(long, help = "Full-text search across name + docstring")]
        search: Option<String>,
        #[arg(long)]
        fuzzy: bool,
        #[arg(long, help = "Return a handle instead of full results")]
        handle: bool,
    },

    /// Index a file or directory for symbol search
    Index { path: String },

    /// Show current claims, symbols, and health
    Status,

    /// Print workflow documentation
    Workflow,

    /// Generate context for the current agent
    Context {
        #[arg(long)]
        task_id: Option<String>,
    },

    /// Update bacchus to the latest version
    SelfUpdate,

    /// Check if a newer version is available
    CheckUpdate,

    /// List recent orchestration events
    Events {
        #[arg(long, default_value = "50")]
        limit: i32,
    },

    /// Manage session state for stop hooks
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// Manage tasks
    Task {
        #[command(subcommand)]
        command: TaskCommands,
    },

    /// Manage epics
    Epic {
        #[command(subcommand)]
        command: EpicCommands,
    },

    /// Agent message bus
    Message {
        #[command(subcommand)]
        command: MessageCommands,
    },

    /// Manage agent archetypes
    Archetype {
        #[command(subcommand)]
        command: ArchetypeCommands,
    },

    /// Token-saving query result handles
    Handle {
        #[command(subcommand)]
        command: HandleCommands,
    },
}

#[derive(Subcommand)]
pub enum SessionCommands {
    /// Start a session
    Start {
        #[arg(value_enum)]
        mode: SessionMode,
        #[arg(long)]
        task_id: Option<String>,
        #[arg(long, default_value = "3")]
        max_concurrent: i32,
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long, help = "Epic ID for orchestrator breadcrumb")]
        epic_id: Option<String>,
        #[arg(long, help = "Goal description for orchestrator breadcrumb")]
        goal: Option<String>,
    },

    /// Stop the current session
    Stop,

    /// Show session status
    Status,

    /// Check if session should block exit (stop hook)
    Check,

    /// Spawn ready workers once
    SpawnWorkers {
        #[arg(long)]
        count: Option<usize>,
        #[arg(long)]
        dry_run: bool,
    },

    /// Remove stale session files and orphaned leases
    Prune {
        #[arg(long, default_value = "240")]
        minutes: i64,
    },

    #[command(hide = true)]
    HeartbeatLoop {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        token: String,
        #[arg(long, default_value = "30000")]
        interval_ms: u64,
    },

    #[command(hide = true)]
    LeaseLoop {
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        token: String,
        #[arg(long, default_value = "30000")]
        interval_ms: u64,
    },

    #[command(hide = true)]
    WorkerRun {
        #[arg(long)]
        worker_id: i64,
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        agent_id: String,
        #[arg(long)]
        scope_id: String,
        #[arg(long)]
        command: String,
    },
}

#[derive(Subcommand)]
pub enum TaskCommands {
    /// List tasks
    List {
        #[arg(long, help = "Filter: open, in_progress, blocked, closed")]
        status: Option<String>,
        #[arg(long, help = "Show only ready tasks")]
        ready: bool,
    },

    /// Show task details
    Show { id: String },

    /// Validate tasks against symbol index
    Validate,

    /// Create tasks.yaml template
    Init,

    /// Import tasks from YAML to SQLite
    Import {
        #[arg(long)]
        epic_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum EpicCommands {
    /// List epics
    List {
        #[arg(long, help = "Filter: open, planning, active, closed")]
        status: Option<String>,
    },

    /// Show epic details with task counts
    Show { id: String },

    /// Create a new epic
    Create {
        #[arg(long)]
        id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: Option<String>,
    },

    /// Assign epic to an architect agent
    Assign { id: String, agent: String },

    /// Update epic status
    SetStatus {
        id: String,
        #[arg(help = "open, planning, active, or closed")]
        status: String,
    },
}

#[derive(Subcommand)]
pub enum MessageCommands {
    /// List messages
    List {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, help = "Filter: pending, processing, processed, failed")]
        status: Option<String>,
    },

    /// Send a message to an agent
    Send {
        agent: String,
        message_type: String,
        payload: String,
    },

    /// Claim pending messages
    Claim {
        agent: String,
        #[arg(long, default_value = "10")]
        limit: i32,
    },

    /// Mark message as processed
    Ack { message_id: i64, agent: String },

    /// Mark message as failed
    Fail {
        message_id: i64,
        agent: String,
        #[arg(long)]
        reason: Option<String>,
    },

    /// Reclaim stale processing messages
    ReclaimStale,
}

#[derive(Subcommand)]
pub enum ArchetypeCommands {
    /// List available archetypes
    List,

    /// Select best archetype for a task
    Select { task_id: String },

    /// Show archetype details
    Show { name: String },

    /// Get archetype prompt text
    Prompt { name: String },
}

#[derive(Subcommand)]
pub enum HandleCommands {
    /// Expand a handle to retrieve its data
    Expand {
        handle: String,
        #[arg(short = 'n', long, default_value = "50")]
        limit: i32,
        #[arg(long, default_value = "0")]
        offset: i32,
    },

    /// Filter a handle by criteria, creating a new handle
    Filter {
        handle: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        file: Option<String>,
    },

    /// List active handles
    List,

    /// Clear all handles
    Clear,

    /// Inspect a handle
    Info { handle: String },
}
