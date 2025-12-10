//! Database module for Bacchus
//!
//! Provides SQLite connection management and migrations.

mod migrations;
mod connection;

pub use connection::{init_db, close_db, with_db};
