//! Structured orchestration event log.
//!
//! Events are persisted for observability, replay, and postmortem analysis.
//! Optional idempotency keys allow exactly-once style writes for critical events.

use crate::db::with_db_str;
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

    with_db_str(|conn| {
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
}

/// Fetch recent orchestration events.
pub fn list_recent_events(limit: i32) -> Result<Vec<OrchestrationEvent>, String> {
    let limit = limit.clamp(1, 500);
    with_db_str(|conn| {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::close_db;
    use crate::testutil::setup_empty_test_db;

    #[test]
    fn test_record_and_list_events() {
        let _dir = setup_empty_test_db();

        let inserted = record_event(
            Some("run-1"),
            "agent",
            "task_started",
            "task",
            "T-001",
            &serde_json::json!({"detail": "started"}),
            None,
        )
        .unwrap();
        assert!(inserted);

        let events = list_recent_events(10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "task_started");
        assert_eq!(events[0].entity_id, "T-001");

        close_db();
    }

    #[test]
    fn test_idempotency_key_prevents_duplicate() {
        let _dir = setup_empty_test_db();
        let payload = serde_json::json!({});

        let first = record_event(
            None,
            "agent",
            "test_event",
            "task",
            "T-002",
            &payload,
            Some("key-1"),
        )
        .unwrap();
        assert!(first);

        let second = record_event(
            None,
            "agent",
            "test_event",
            "task",
            "T-002",
            &payload,
            Some("key-1"),
        )
        .unwrap();
        assert!(!second);

        let events = list_recent_events(10).unwrap();
        assert_eq!(events.len(), 1);

        close_db();
    }

    #[test]
    fn test_list_recent_events_respects_limit() {
        let _dir = setup_empty_test_db();
        let payload = serde_json::json!({});

        for i in 0..5 {
            record_event(
                None,
                "agent",
                "test_event",
                "task",
                &format!("T-{}", i),
                &payload,
                None,
            )
            .unwrap();
        }

        let events = list_recent_events(3).unwrap();
        assert_eq!(events.len(), 3);

        close_db();
    }
}
