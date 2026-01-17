//! Configuration management for Bacchus
//!
//! Supports environment variable overrides for database and worktree paths.
//!
//! # Environment Variables
//!
//! - `BACCHUS_DB_PATH`: Override path to bacchus database (default: `.bacchus/bacchus.db`)
//! - `BACCHUS_WORKTREES`: Override path to worktrees directory (default: `.bacchus/worktrees`)
//!
//! These environment variables are checked directly in their respective modules:
//! - `BACCHUS_DB_PATH` in `main.rs`
//! - `BACCHUS_WORKTREES` in `worktree.rs`
