//! Structured orchestration event log.
//!
//! Events are persisted for observability, replay, and postmortem analysis.
//! Optional idempotency keys allow exactly-once style writes for critical events.

use crate::db::with_db;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationEvent {
    pub id: i64,
    pub run_id: Option<String>,
    pub actor: String,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub payload: Value,
    pub created_at: i64,
    pub idempotency_key: Option<String>,
}

/// Record an event.
///
/// Returns:
/// - `Ok(true)` when inserted
/// - `Ok(false)` when skipped due to idempotency-key duplicate
pub fn record_event(
    run_id: Option<&str>,
    actor: &str,
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    payload: &Value,
    idempotency_key: Option<&str>,
) -> Result<bool, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let payload_str = payload.to_string();

    with_db(|conn| {
        let result = conn.execute(
            "INSERT INTO orchestration_events
             (run_id, actor, event_type, entity_type, entity_id, payload, created_at, idempotency_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                run_id,
                actor,
                event_type,
                entity_type,
                entity_id,
                payload_str,
                now,
                idempotency_key
            ],
        );

        match result {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
            {
                Ok(false)
            }
            Err(e) => Err(e),
        }
    })
    .map_err(|e: rusqlite::Error| e.to_string())
}

/// Fetch recent orchestration events.
pub fn list_recent_events(limit: i32) -> Result<Vec<OrchestrationEvent>, String> {
    let limit = limit.clamp(1, 500);
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, actor, event_type, entity_type, entity_id, payload, created_at, idempotency_key
             FROM orchestration_events
             ORDER BY created_at DESC
             LIMIT ?1",
        )?;

        let events = stmt
            .query_map([limit], |row| {
                let payload_str: String = row.get(6)?;
                Ok(OrchestrationEvent {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    actor: row.get(2)?,
                    event_type: row.get(3)?,
                    entity_type: row.get(4)?,
                    entity_id: row.get(5)?,
                    payload: serde_json::from_str(&payload_str).unwrap_or(Value::Null),
                    created_at: row.get(7)?,
                    idempotency_key: row.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(events)
    })
    .map_err(|e: rusqlite::Error| e.to_string())
}
