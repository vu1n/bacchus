//! Configuration management for Bacchus
//!
//! Supports environment variable overrides for database and workspace paths.
//!
//! # Environment Variables
//!
//! - `BACCHUS_DB_PATH`: Override path to bacchus database (default: `.bacchus/bacchus.db`)
//! - `BACCHUS_WORKSPACES`: Override path to workspaces directory (default: `.bacchus/workspaces`)
//!
//! These environment variables are checked directly in their respective modules:
//! - `BACCHUS_DB_PATH` in `main.rs`
//! - `BACCHUS_WORKSPACES` in `workspace.rs`
