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

    /// Get next ready bead, create worktree, claim it
    Next {
        /// Your agent ID
        agent_id: String,
    },

    /// Release a claimed bead
    Release {
        /// The bead ID to release
        bead_id: String,
        /// Release status: done (merge), blocked (keep), or failed (discard)
        #[arg(long, default_value = "done")]
        status: String,
    },

    /// Find stale claims and optionally clean them up
    Stale {
        /// Minutes without activity to consider stale
        #[arg(short, long, default_value = "15")]
        minutes: i64,
        /// Clean up stale claims (remove worktrees, reset beads)
        #[arg(long)]
        cleanup: bool,
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
}
