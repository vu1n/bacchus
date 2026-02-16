//! Agent message queue module
//!
//! Provides pull-based communication between agents via SQLite.
//! Messages are claimed atomically to prevent double-processing.

use crate::db::with_db;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// Types
// ============================================================================

/// Message status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Pending,
    Processing,
    Processed,
    Failed,
}

impl MessageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageStatus::Pending => "pending",
            MessageStatus::Processing => "processing",
            MessageStatus::Processed => "processed",
            MessageStatus::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, MessagesError> {
        match s {
            "pending" => Ok(MessageStatus::Pending),
            "processing" => Ok(MessageStatus::Processing),
            "processed" => Ok(MessageStatus::Processed),
            "failed" => Ok(MessageStatus::Failed),
            _ => Err(MessagesError::InvalidStatus(s.to_string())),
        }
    }
}

impl std::fmt::Display for MessageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// An agent message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: i64,
    pub target_agent: String,
    pub message_type: String,
    pub payload: serde_json::Value,
    pub status: MessageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processing_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked_at: Option<i64>,
    pub attempts: i32,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed_at: Option<i64>,
}

/// Input for sending a new message
#[derive(Debug, Clone)]
pub struct SendMessageInput {
    pub target_agent: String,
    pub message_type: String,
    pub payload: serde_json::Value,
}

/// Constants for message processing
pub const PROCESSING_TIMEOUT_MS: i64 = 300_000; // 5 minutes
pub const MAX_ATTEMPTS: i32 = 3;

/// Errors that can occur when working with messages
#[derive(Debug, Error)]
pub enum MessagesError {
    #[error("Message not found: {0}")]
    NotFound(i64),

    #[error("Invalid status: {0}")]
    InvalidStatus(String),

    #[error("Database error: {0}")]
    DbError(String),
}

impl From<rusqlite::Error> for MessagesError {
    fn from(e: rusqlite::Error) -> Self {
        MessagesError::DbError(e.to_string())
    }
}

// ============================================================================
// Message Operations
// ============================================================================

/// Send a new message to an agent
pub fn send_message(input: SendMessageInput) -> Result<AgentMessage, MessagesError> {
    let now = chrono::Utc::now().timestamp_millis();
    let payload_str = input.payload.to_string();

    with_db(|conn| {
        conn.execute(
            "INSERT INTO agent_messages (target_agent, message_type, payload, status, created_at)
             VALUES (?1, ?2, ?3, 'pending', ?4)",
            params![input.target_agent, input.message_type, payload_str, now],
        )?;

        let id = conn.last_insert_rowid();

        Ok(AgentMessage {
            id,
            target_agent: input.target_agent,
            message_type: input.message_type,
            payload: input.payload,
            status: MessageStatus::Pending,
            processing_by: None,
            locked_at: None,
            attempts: 0,
            created_at: now,
            processed_at: None,
        })
    })
    .map_err(|e: rusqlite::Error| MessagesError::DbError(e.to_string()))
}

/// Claim pending messages for an agent atomically
///
/// Returns up to `limit` messages, marking them as 'processing'.
/// Uses atomic UPDATE ... WHERE to prevent race conditions.
pub fn claim_messages(agent_id: &str, limit: i32) -> Result<Vec<AgentMessage>, MessagesError> {
    let now = chrono::Utc::now().timestamp_millis();
    let limit = limit.clamp(1, 100);

    with_db(|conn| {
        // Use savepoint for auto-rollback on error
        conn.execute("SAVEPOINT claim_messages", [])?;

        let result = (|| -> rusqlite::Result<Vec<AgentMessage>> {
            // Get IDs of messages to claim
            let mut stmt = conn.prepare(
                "SELECT id FROM agent_messages
                 WHERE target_agent = ?1 AND status = 'pending'
                 ORDER BY created_at
                 LIMIT ?2"
            )?;

            let ids: Vec<i64> = stmt
                .query_map(params![agent_id, limit], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();

            if ids.is_empty() {
                return Ok(Vec::new());
            }

            // Update claimed messages - include status='pending' check to handle contention
            // If another agent claimed a message between SELECT and UPDATE, we skip it cleanly
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "UPDATE agent_messages
                 SET status = 'processing', processing_by = ?1, locked_at = ?2, attempts = attempts + 1
                 WHERE id IN ({}) AND status = 'pending'",
                placeholders
            );

            conn.execute(
                &sql,
                rusqlite::params_from_iter(
                    std::iter::once(&agent_id as &dyn rusqlite::ToSql)
                        .chain(std::iter::once(&now as &dyn rusqlite::ToSql))
                        .chain(ids.iter().map(|id| id as &dyn rusqlite::ToSql))
                ),
            )?;

            // Fetch only the messages we actually claimed (status='processing' AND processing_by=us)
            // This handles cases where another agent claimed some messages between SELECT and UPDATE
            let mut messages = Vec::new();
            for id in ids {
                if let Ok(msg) = conn.query_row(
                    "SELECT id, target_agent, message_type, payload, status, processing_by, locked_at, attempts, created_at, processed_at
                     FROM agent_messages WHERE id = ?1 AND status = 'processing' AND processing_by = ?2",
                    params![id, agent_id],
                    |row| {
                        let status_str: String = row.get(4)?;
                        let payload_str: String = row.get(3)?;
                        Ok(AgentMessage {
                            id: row.get(0)?,
                            target_agent: row.get(1)?,
                            message_type: row.get(2)?,
                            payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null),
                            status: MessageStatus::from_str(&status_str).unwrap_or(MessageStatus::Pending),
                            processing_by: row.get(5)?,
                            locked_at: row.get(6)?,
                            attempts: row.get(7)?,
                            created_at: row.get(8)?,
                            processed_at: row.get(9)?,
                        })
                    },
                ) {
                    messages.push(msg);
                }
                // If query_row fails (message claimed by another agent), we skip it silently
            }

            Ok(messages)
        })();

        match result {
            Ok(messages) => {
                conn.execute("RELEASE claim_messages", [])?;
                Ok(messages)
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK TO claim_messages", []);
                let _ = conn.execute("RELEASE claim_messages", []);
                Err(e)
            }
        }
    })
    .map_err(|e: rusqlite::Error| MessagesError::DbError(e.to_string()))
}

/// Mark a message as processed
pub fn mark_processed(message_id: i64, agent_id: &str) -> Result<(), MessagesError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE agent_messages
             SET status = 'processed', processed_at = ?1, processing_by = NULL, locked_at = NULL
             WHERE id = ?2 AND processing_by = ?3",
            params![now, message_id, agent_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!(
                    "Message {} not found or not owned by {}",
                    message_id, agent_id
                )),
            ));
        }

        Ok(())
    })
    .map_err(|e: rusqlite::Error| {
        if e.to_string().contains("not found or not owned") {
            MessagesError::NotFound(message_id)
        } else {
            MessagesError::DbError(e.to_string())
        }
    })
}

/// Mark a message as failed
pub fn mark_failed(
    message_id: i64,
    agent_id: &str,
    reason: Option<&str>,
) -> Result<(), MessagesError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db(|conn| {
        let affected = conn.execute(
            "UPDATE agent_messages
             SET status = 'failed', processed_at = ?1, processing_by = NULL, locked_at = NULL
             WHERE id = ?2 AND processing_by = ?3",
            params![now, message_id, agent_id],
        )?;

        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!(
                    "Message {} not found or not owned by {}",
                    message_id, agent_id
                )),
            ));
        }

        if let Some(msg) = reason {
            eprintln!(
                "Message {} marked failed by {}: {}",
                message_id, agent_id, msg
            );
        }

        Ok(())
    })
    .map_err(|e: rusqlite::Error| {
        if e.to_string().contains("not found or not owned") {
            MessagesError::NotFound(message_id)
        } else {
            MessagesError::DbError(e.to_string())
        }
    })
}

/// List messages with optional filters
pub fn list_messages(
    target_agent: Option<&str>,
    status: Option<MessageStatus>,
) -> Result<Vec<AgentMessage>, MessagesError> {
    with_db(|conn| {
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(agent) = target_agent {
            conditions.push(format!("target_agent = ?{}", params.len() + 1));
            params.push(Box::new(agent.to_string()));
        }

        if let Some(s) = status {
            conditions.push(format!("status = ?{}", params.len() + 1));
            params.push(Box::new(s.as_str().to_string()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT id, target_agent, message_type, payload, status, processing_by, locked_at, attempts, created_at, processed_at
             FROM agent_messages {} ORDER BY created_at DESC",
            where_clause
        );

        let mut stmt = conn.prepare(&sql)?;

        let messages = stmt
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                |row| {
                    let status_str: String = row.get(4)?;
                    let payload_str: String = row.get(3)?;
                    Ok(AgentMessage {
                        id: row.get(0)?,
                        target_agent: row.get(1)?,
                        message_type: row.get(2)?,
                        payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null),
                        status: MessageStatus::from_str(&status_str).unwrap_or(MessageStatus::Pending),
                        processing_by: row.get(5)?,
                        locked_at: row.get(6)?,
                        attempts: row.get(7)?,
                        created_at: row.get(8)?,
                        processed_at: row.get(9)?,
                    })
                },
            )?
            .filter_map(|r| r.ok())
            .collect();

        Ok(messages)
    })
    .map_err(|e: rusqlite::Error| MessagesError::DbError(e.to_string()))
}

/// Reclaim stale messages (called by orchestrator)
///
/// Messages stuck in 'processing' for longer than PROCESSING_TIMEOUT_MS are:
/// - Requeued to 'pending' if attempts < MAX_ATTEMPTS
/// - Marked as 'failed' if attempts >= MAX_ATTEMPTS
///
/// Returns (requeued_count, failed_count)
pub fn reclaim_stale_messages() -> Result<(usize, usize), MessagesError> {
    let now = chrono::Utc::now().timestamp_millis();
    let timeout_threshold = now - PROCESSING_TIMEOUT_MS;

    with_db(|conn| {
        // Requeue messages under attempt limit
        let requeued = conn.execute(
            "UPDATE agent_messages
             SET status = 'pending', processing_by = NULL, locked_at = NULL, processed_at = NULL
             WHERE status = 'processing' AND locked_at < ?1 AND attempts < ?2",
            params![timeout_threshold, MAX_ATTEMPTS],
        )?;

        // Fail messages over attempt limit
        let failed = conn.execute(
            "UPDATE agent_messages
             SET status = 'failed', processing_by = NULL, locked_at = NULL, processed_at = ?1
             WHERE status = 'processing' AND locked_at < ?2 AND attempts >= ?3",
            params![now, timeout_threshold, MAX_ATTEMPTS],
        )?;

        Ok((requeued, failed))
    })
    .map_err(|e: rusqlite::Error| MessagesError::DbError(e.to_string()))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use tempfile::tempdir;

    fn setup_test_db() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap())).unwrap();
        dir
    }

    #[test]
    fn test_send_message() {
        let _dir = setup_test_db();

        let input = SendMessageInput {
            target_agent: "architect-1".to_string(),
            message_type: "epic_assigned".to_string(),
            payload: serde_json::json!({"epic_id": "EPIC-001"}),
        };

        let msg = send_message(input).unwrap();
        assert_eq!(msg.target_agent, "architect-1");
        assert_eq!(msg.message_type, "epic_assigned");
        assert_eq!(msg.status, MessageStatus::Pending);

        crate::db::close_db();
    }

    #[test]
    fn test_claim_messages() {
        let _dir = setup_test_db();

        // Send multiple messages
        for i in 1..=3 {
            let input = SendMessageInput {
                target_agent: "architect-1".to_string(),
                message_type: format!("type_{}", i),
                payload: serde_json::json!({"index": i}),
            };
            send_message(input).unwrap();
        }

        // Claim 2 messages
        let claimed = claim_messages("architect-1", 2).unwrap();
        assert_eq!(claimed.len(), 2);
        assert!(claimed
            .iter()
            .all(|m| m.status == MessageStatus::Processing));
        assert!(claimed
            .iter()
            .all(|m| m.processing_by == Some("architect-1".to_string())));

        // Verify only 1 pending remains
        let remaining = list_messages(Some("architect-1"), Some(MessageStatus::Pending)).unwrap();
        assert_eq!(remaining.len(), 1);

        crate::db::close_db();
    }

    #[test]
    fn test_mark_processed() {
        let _dir = setup_test_db();

        let input = SendMessageInput {
            target_agent: "worker-1".to_string(),
            message_type: "task_assigned".to_string(),
            payload: serde_json::json!({}),
        };

        let _msg = send_message(input).unwrap();
        let claimed = claim_messages("worker-1", 1).unwrap();
        assert_eq!(claimed.len(), 1);

        mark_processed(claimed[0].id, "worker-1").unwrap();

        let processed = list_messages(Some("worker-1"), Some(MessageStatus::Processed)).unwrap();
        assert_eq!(processed.len(), 1);
        assert!(processed[0].processing_by.is_none());

        crate::db::close_db();
    }

    #[test]
    fn test_mark_failed() {
        let _dir = setup_test_db();

        let input = SendMessageInput {
            target_agent: "worker-2".to_string(),
            message_type: "task_assigned".to_string(),
            payload: serde_json::json!({}),
        };

        let _msg = send_message(input).unwrap();
        let claimed = claim_messages("worker-2", 1).unwrap();
        assert_eq!(claimed.len(), 1);

        mark_failed(claimed[0].id, "worker-2", Some("validation failed")).unwrap();

        let failed = list_messages(Some("worker-2"), Some(MessageStatus::Failed)).unwrap();
        assert_eq!(failed.len(), 1);
        assert!(failed[0].processing_by.is_none());

        crate::db::close_db();
    }

    #[test]
    fn test_list_messages_filtered() {
        let _dir = setup_test_db();

        // Send messages to different agents
        for agent in &["agent-1", "agent-2"] {
            let input = SendMessageInput {
                target_agent: agent.to_string(),
                message_type: "test".to_string(),
                payload: serde_json::json!({}),
            };
            send_message(input).unwrap();
        }

        let agent1_msgs = list_messages(Some("agent-1"), None).unwrap();
        assert_eq!(agent1_msgs.len(), 1);

        let all_msgs = list_messages(None, None).unwrap();
        assert_eq!(all_msgs.len(), 2);

        crate::db::close_db();
    }

    #[test]
    fn test_reclaim_stale_no_stale() {
        let _dir = setup_test_db();

        let input = SendMessageInput {
            target_agent: "agent-1".to_string(),
            message_type: "test".to_string(),
            payload: serde_json::json!({}),
        };
        send_message(input).unwrap();
        claim_messages("agent-1", 1).unwrap();

        // No stale messages yet (just claimed)
        let (requeued, failed) = reclaim_stale_messages().unwrap();
        assert_eq!(requeued, 0);
        assert_eq!(failed, 0);

        crate::db::close_db();
    }
}
