//! Database module for Bacchus
//!
//! Provides SQLite connection management and migrations.

mod connection;
mod migrations;

pub use connection::{close_db, init_db, with_db};
