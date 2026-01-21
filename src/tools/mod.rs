//! Tool implementations for Bacchus
//!
//! Each tool corresponds to a CLI command.

pub mod archetypes;
pub mod context;
pub mod claim;
pub mod eval;
pub mod list;
pub mod next;
pub mod release;
pub mod resolve;
pub mod abort;
pub mod review;
pub mod session;
pub mod stale;
pub mod symbols;
pub mod task_commands;

pub use archetypes::{cmd_list_archetypes, cmd_show_archetype, cmd_archetype_prompt, cmd_select_archetype};
pub use context::generate_context;
pub use claim::claim_task;
pub use eval::{record_event, generate_eval_report, EventType};
pub use list::list_claims;
pub use next::next_task;
pub use release::release_task;
pub use resolve::resolve_merge;
pub use abort::abort_merge;
pub use review::review_task;
pub use session::{start_session, stop_session, session_status, check_session};
pub use stale::find_stale;
pub use symbols::{find_symbols, FindSymbolsInput};
pub use task_commands::{list_tasks, show_task, validate_tasks, init_tasks, import_tasks};

