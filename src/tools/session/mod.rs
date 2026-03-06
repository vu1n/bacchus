//! Session management for stop hooks
//!
//! Manages scoped session files under .bacchus/sessions/ for persistent session state.

mod config;
mod file;
mod heartbeat;
mod hooks;
mod lifecycle;
mod server;
pub mod types;
mod workers;

// Re-export public API to maintain backward compatibility.
pub use heartbeat::{
    attach_agent_session_heartbeat, run_agent_heartbeat_loop, run_orchestrator_lease_loop,
};
pub use hooks::check_session;
pub use lifecycle::{prune_sessions, session_status, start_session, stop_session};
pub use types::SessionMode;
pub use workers::{run_worker_command, spawn_workers_once};
