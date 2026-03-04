//! Database module for Bacchus
//!
//! Provides SQLite connection management and schema initialization.

mod connection;
mod schema;

pub use connection::{close_db, init_db, with_db, with_db_str, with_db_typed, with_savepoint};

/// Current Unix epoch timestamp in milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
