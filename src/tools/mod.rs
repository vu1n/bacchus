//! Tool implementations for Bacchus
//!
//! Each tool corresponds to a CLI command.

pub mod abort;
pub mod archetypes;
pub mod claim;
pub mod context;
pub mod eval;
pub mod list;
pub mod next;
pub mod orchestrator;
pub mod release;
pub mod resolve;
pub mod review;
pub mod session;
pub mod stale;
pub mod symbols;
pub mod task_commands;

pub use abort::abort_merge;
pub use archetypes::{
    cmd_archetype_prompt, cmd_list_archetypes, cmd_select_archetype, cmd_show_archetype,
};
pub use claim::claim_task;
pub use context::generate_context;
pub use eval::generate_eval_report;
pub use list::list_claims;
pub use next::next_task;
pub use orchestrator::process_ready_releases;
pub use release::release_task;
pub use resolve::resolve_merge;
pub use review::review_task;
pub use session::{
    check_session, run_agent_heartbeat_loop, run_orchestrator_lease_loop, session_status,
    start_session, stop_session,
};
pub use stale::find_stale;
pub use symbols::{find_symbols, find_symbols_handle, FindSymbolsInput};
pub use task_commands::{import_tasks, init_tasks, list_tasks, show_task, validate_tasks};
