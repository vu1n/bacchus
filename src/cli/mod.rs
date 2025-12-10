//! CLI module for Bacchus
//!
//! Defines command-line interface using clap.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bacchus")]
#[command(about = "AST-aware coordination CLI for multi-agent work")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    // ========================================================================
    // Coordination Commands
    // ========================================================================

    /// Claim ownership of a task/bead
    Claim {
        /// The bead ID to claim
        bead_id: String,
        /// Your agent ID
        agent_id: String,
    },

    /// Release a claimed task
    Release {
        /// The bead ID to release
        bead_id: String,
        /// Your agent ID
        agent_id: String,
        /// Reason for release
        #[arg(short, long)]
        reason: Option<String>,
    },

    /// Declare what symbols you plan to modify/create
    Workplan {
        /// The bead ID
        bead_id: String,
        /// Your agent ID
        agent_id: String,
        /// Comma-separated list of files to modify
        #[arg(long)]
        modifies_files: Option<String>,
        /// Comma-separated list of symbols to modify
        #[arg(long)]
        modifies_symbols: Option<String>,
        /// Comma-separated list of modules to modify
        #[arg(long)]
        modifies_modules: Option<String>,
        /// Comma-separated list of symbols to create
        #[arg(long)]
        creates_symbols: Option<String>,
    },

    /// Report actual changes made
    Footprint {
        /// The bead ID
        bead_id: String,
        /// Your agent ID
        agent_id: String,
        /// Comma-separated list of changed files
        #[arg(long)]
        files: String,
        /// Total lines added
        #[arg(long)]
        added: Option<i32>,
        /// Total lines removed
        #[arg(long)]
        removed: Option<i32>,
        /// Breaking changes (format: symbol:kind:description, comma-separated)
        #[arg(long)]
        breaking: Option<String>,
    },

    /// Keep task alive and get pending notifications
    Heartbeat {
        /// The bead ID
        bead_id: String,
        /// Your agent ID
        agent_id: String,
    },

    /// Find tasks that haven't had a heartbeat recently
    Stale {
        /// Minutes without heartbeat to consider stale
        #[arg(short, long, default_value = "15")]
        minutes: i64,
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
    },

    /// Get code context bundle for a task
    Context {
        /// The bead ID
        bead_id: String,
        /// Max tokens for context
        #[arg(short, long, default_value = "8000")]
        tokens: i32,
    },

    /// Index a file or directory
    Index {
        /// Path to file or directory to index
        path: String,
    },

    // ========================================================================
    // Communication Commands
    // ========================================================================

    /// Get notifications for an agent
    Notifications {
        /// Your agent ID
        agent_id: String,
        /// Filter by status (pending, acknowledged, resolved)
        #[arg(short, long)]
        status: Option<String>,
        /// Max notifications
        #[arg(short = 'n', long, default_value = "20")]
        limit: i32,
    },

    /// Acknowledge or resolve a notification
    Resolve {
        /// The notification ID
        notification_id: i64,
        /// Your agent ID
        agent_id: String,
        /// Action: acknowledge or resolve
        action: String,
        /// Optional notes
        #[arg(long)]
        notes: Option<String>,
    },

    /// Find who cares about a symbol
    Stakeholders {
        /// The symbol to query
        symbol: String,
        /// Include transitive dependencies
        #[arg(short, long)]
        transitive: bool,
    },

    /// Send notification to stakeholders of a symbol
    Notify {
        /// The symbol
        symbol: String,
        /// Your agent ID
        agent_id: String,
        /// Your bead ID
        bead_id: String,
        /// Change kind (signature, behavior, removal, rename)
        kind: String,
        /// Description of the change
        description: String,
        /// Git commit hash
        #[arg(short, long)]
        commit: Option<String>,
    },

    // ========================================================================
    // Human Escalation Commands
    // ========================================================================

    /// Request a human decision
    Decide {
        /// Your agent ID
        agent_id: String,
        /// The bead ID
        bead_id: String,
        /// The question for the human
        question: String,
        /// Comma-separated options
        #[arg(short, long)]
        options: String,
        /// Additional context
        #[arg(short, long)]
        context: Option<String>,
        /// Comma-separated affected symbols
        #[arg(long)]
        symbols: Option<String>,
        /// Urgency: low, medium, high
        #[arg(short, long, default_value = "medium")]
        urgency: String,
    },

    /// Submit a human decision
    Answer {
        /// The notification ID
        notification_id: i64,
        /// Your human identifier
        human_id: String,
        /// The chosen option
        decision: String,
        /// Notes explaining the decision
        #[arg(long)]
        notes: Option<String>,
    },

    /// Get pending human decisions
    Pending {
        /// Filter by human ID
        #[arg(short = 'H', long)]
        human: Option<String>,
        /// Max results
        #[arg(short = 'n', long, default_value = "20")]
        limit: i32,
    },

    // ========================================================================
    // Info Commands
    // ========================================================================

    /// Show current tasks, owners, notification counts
    Status,

    /// Print protocol documentation
    Workflow,
}
