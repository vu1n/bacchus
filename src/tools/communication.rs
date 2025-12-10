//! Communication tools for notifications and stakeholder queries

use crate::db::with_db;
use rusqlite::{params, Result};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

// ============================================================================
// Input/Output Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct GetNotificationsInput {
    pub agent_id: String,
    pub status: Option<String>,
    pub limit: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetNotificationsOutput {
    pub notifications: Vec<Notification>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub notification_type: String,
    pub from_agent: Option<String>,
    pub from_bead: Option<String>,
    pub target_symbol: Option<String>,
    pub change_kind: Option<String>,
    pub change_description: Option<String>,
    pub decision_options: Option<String>,
    pub status: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveNotificationInput {
    pub notification_id: i64,
    pub agent_id: String,
    pub action: String, // "acknowledge" or "resolve"
    pub notes: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResolveNotificationOutput {
    pub success: bool,
    pub notification_id: i64,
    pub new_status: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryStakeholdersInput {
    pub symbol: String,
    pub include_transitive: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QueryStakeholdersOutput {
    pub direct_stakeholders: Vec<Stakeholder>,
    pub transitive_stakeholders: Vec<Stakeholder>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Stakeholder {
    pub agent_id: Option<String>,
    pub bead_id: String,
    pub relation: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestHumanDecisionInput {
    pub agent_id: String,
    pub bead_id: String,
    pub question: String,
    pub options: Vec<String>,
    pub context: Option<String>,
    pub affected_symbols: Option<Vec<String>>,
    pub urgency: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestHumanDecisionOutput {
    pub notification_id: i64,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitHumanDecisionInput {
    pub notification_id: i64,
    pub human_id: String,
    pub decision: String,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitHumanDecisionOutput {
    pub success: bool,
    pub notification_id: i64,
    pub decision: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetPendingDecisionsInput {
    pub human_id: Option<String>,
    pub limit: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetPendingDecisionsOutput {
    pub pending_decisions: Vec<PendingDecision>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PendingDecision {
    pub id: i64,
    pub from_agent: Option<String>,
    pub from_bead: Option<String>,
    pub question: String,
    pub options: Vec<String>,
    pub context: Option<String>,
    pub urgency: String,
    pub created_at: i64,
}

// ============================================================================
// Tool Implementations
// ============================================================================

/// Get notifications for an agent
pub fn get_notifications(input: &GetNotificationsInput) -> Result<GetNotificationsOutput> {
    with_db(|conn| {
        let status_filter = input.status.as_deref().unwrap_or("pending");
        let limit = input.limit.unwrap_or(20);

        let mut stmt = conn.prepare(
            "SELECT id, notification_type, from_agent, from_bead, target_symbol, change_kind, change_description, decision_options, status, created_at FROM notifications WHERE target_agent = ?1 AND status = ?2 ORDER BY created_at DESC LIMIT ?3"
        )?;

        let notifications: Vec<Notification> = stmt
            .query_map(params![&input.agent_id, status_filter, limit], |row| {
                Ok(Notification {
                    id: row.get(0)?,
                    notification_type: row.get(1)?,
                    from_agent: row.get(2)?,
                    from_bead: row.get(3)?,
                    target_symbol: row.get(4)?,
                    change_kind: row.get(5)?,
                    change_description: row.get(6)?,
                    decision_options: row.get(7)?,
                    status: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(GetNotificationsOutput { notifications })
    })
}

/// Acknowledge or resolve a notification
pub fn resolve_notification(input: &ResolveNotificationInput) -> Result<ResolveNotificationOutput> {
    let now = current_timestamp();

    with_db(|conn| {
        // Verify notification exists and belongs to agent
        let target_agent: Option<Option<String>> = conn
            .query_row(
                "SELECT target_agent FROM notifications WHERE id = ?1",
                [input.notification_id],
                |row| row.get(0),
            )
            .ok();

        let Some(agent) = target_agent else {
            return Ok(ResolveNotificationOutput {
                success: false,
                notification_id: input.notification_id,
                new_status: "unknown".to_string(),
                message: "Notification not found".to_string(),
            });
        };

        // agent is Option<String> (target_agent column can be null)
        let agent_str = agent.unwrap_or_default();
        if agent_str != input.agent_id {
            return Ok(ResolveNotificationOutput {
                success: false,
                notification_id: input.notification_id,
                new_status: "unchanged".to_string(),
                message: format!("Notification belongs to {}, not {}", agent_str, input.agent_id),
            });
        }

        let (new_status, timestamp_field) = match input.action.as_str() {
            "acknowledge" => ("acknowledged", "acknowledged_at"),
            "resolve" => ("resolved", "resolved_at"),
            _ => {
                return Ok(ResolveNotificationOutput {
                    success: false,
                    notification_id: input.notification_id,
                    new_status: "unchanged".to_string(),
                    message: format!("Invalid action: {}. Use 'acknowledge' or 'resolve'", input.action),
                });
            }
        };

        let sql = format!(
            "UPDATE notifications SET status = ?1, {} = ?2, decision_notes = COALESCE(?3, decision_notes) WHERE id = ?4",
            timestamp_field
        );

        conn.execute(&sql, params![new_status, now, &input.notes, input.notification_id])?;

        Ok(ResolveNotificationOutput {
            success: true,
            notification_id: input.notification_id,
            new_status: new_status.to_string(),
            message: format!("Notification {}", new_status),
        })
    })
}

/// Query stakeholders for a symbol
pub fn query_stakeholders(input: &QueryStakeholdersInput) -> Result<QueryStakeholdersOutput> {
    with_db(|conn| {
        // Direct stakeholders
        let mut stmt = conn.prepare(
            "SELECT DISTINCT t.owner, bs.bead_id, bs.relation FROM bead_symbols bs LEFT JOIN tasks t ON bs.bead_id = t.bead_id WHERE bs.symbol_ref = ?1"
        )?;

        let direct_stakeholders: Vec<Stakeholder> = stmt
            .query_map([&input.symbol], |row| {
                Ok(Stakeholder {
                    agent_id: row.get(0)?,
                    bead_id: row.get(1)?,
                    relation: row.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut transitive_stakeholders = Vec::new();

        // Transitive stakeholders (through symbol_calls)
        if input.include_transitive.unwrap_or(false) {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT t.owner, bs.bead_id, 'calls' as relation FROM symbol_calls sc JOIN bead_symbols bs ON sc.callee_fq_name = bs.symbol_ref LEFT JOIN tasks t ON bs.bead_id = t.bead_id WHERE sc.callee_fq_name = ?1"
            )?;

            transitive_stakeholders = stmt
                .query_map([&input.symbol], |row| {
                    Ok(Stakeholder {
                        agent_id: row.get(0)?,
                        bead_id: row.get(1)?,
                        relation: row.get(2)?,
                    })
                })?
                .filter_map(|r| r.ok())
                .collect();
        }

        Ok(QueryStakeholdersOutput {
            direct_stakeholders,
            transitive_stakeholders,
        })
    })
}

/// Notify stakeholders of a symbol change
pub fn notify_stakeholders(
    symbol: &str,
    agent_id: &str,
    bead_id: &str,
    change_kind: &str,
    description: &str,
    commit_hash: Option<&str>,
) -> Result<i32> {
    let now = current_timestamp();

    with_db(|conn| {
        // Find stakeholders
        let mut stmt = conn.prepare(
            "SELECT DISTINCT t.owner, bs.bead_id FROM bead_symbols bs JOIN tasks t ON bs.bead_id = t.bead_id WHERE bs.symbol_ref = ?1 AND bs.bead_id != ?2 AND t.owner IS NOT NULL"
        )?;

        let stakeholders: Vec<(String, String)> = stmt
            .query_map(params![symbol, bead_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut count = 0;
        for (owner, target_bead) in stakeholders {
            conn.execute(
                "INSERT INTO notifications (notification_type, from_agent, from_bead, commit_hash, target_agent, target_bead, target_symbol, change_kind, change_description, is_breaking, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, 'pending', ?10)",
                params![
                    "breaking_change",
                    agent_id,
                    bead_id,
                    commit_hash,
                    owner,
                    target_bead,
                    symbol,
                    change_kind,
                    description,
                    now
                ],
            )?;
            count += 1;
        }

        Ok(count)
    })
}

/// Request a human decision
pub fn request_human_decision(input: &RequestHumanDecisionInput) -> Result<RequestHumanDecisionOutput> {
    let now = current_timestamp();

    with_db(|conn| {
        let options_json = serde_json::to_string(&input.options).unwrap_or_default();
        let affected_json = input.affected_symbols.as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default());
        let urgency = input.urgency.as_deref().unwrap_or("medium");

        conn.execute(
            "INSERT INTO notifications (notification_type, from_agent, from_bead, change_description, decision_options, status, created_at) VALUES ('human_decision', ?1, ?2, ?3, ?4, 'pending', ?5)",
            params![
                &input.agent_id,
                &input.bead_id,
                &input.question,
                options_json,
                now
            ],
        )?;

        let notification_id = conn.last_insert_rowid();

        Ok(RequestHumanDecisionOutput {
            notification_id,
            status: "pending".to_string(),
            message: "Human decision requested".to_string(),
        })
    })
}

/// Submit a human decision
pub fn submit_human_decision(input: &SubmitHumanDecisionInput) -> Result<SubmitHumanDecisionOutput> {
    let now = current_timestamp();

    with_db(|conn| {
        // Verify notification exists and is a human decision
        let notification_type: Option<String> = conn
            .query_row(
                "SELECT notification_type FROM notifications WHERE id = ?1",
                [input.notification_id],
                |row| row.get(0),
            )
            .ok();

        let Some(ntype) = notification_type else {
            return Ok(SubmitHumanDecisionOutput {
                success: false,
                notification_id: input.notification_id,
                decision: input.decision.clone(),
                message: "Notification not found".to_string(),
            });
        };

        if ntype != "human_decision" {
            return Ok(SubmitHumanDecisionOutput {
                success: false,
                notification_id: input.notification_id,
                decision: input.decision.clone(),
                message: "Not a human decision notification".to_string(),
            });
        }

        conn.execute(
            "UPDATE notifications SET decision_result = ?1, decision_notes = ?2, status = 'resolved', resolved_at = ?3 WHERE id = ?4",
            params![&input.decision, &input.notes, now, input.notification_id],
        )?;

        Ok(SubmitHumanDecisionOutput {
            success: true,
            notification_id: input.notification_id,
            decision: input.decision.clone(),
            message: "Decision submitted".to_string(),
        })
    })
}

/// Get pending human decisions
pub fn get_pending_decisions(input: &GetPendingDecisionsInput) -> Result<GetPendingDecisionsOutput> {
    with_db(|conn| {
        let limit = input.limit.unwrap_or(20);

        let mut stmt = conn.prepare(
            "SELECT id, from_agent, from_bead, change_description, decision_options, created_at FROM notifications WHERE notification_type = 'human_decision' AND status = 'pending' ORDER BY created_at DESC LIMIT ?1"
        )?;

        let pending_decisions: Vec<PendingDecision> = stmt
            .query_map([limit], |row| {
                let options_json: String = row.get(4)?;
                let options: Vec<String> = serde_json::from_str(&options_json).unwrap_or_default();

                Ok(PendingDecision {
                    id: row.get(0)?,
                    from_agent: row.get(1)?,
                    from_bead: row.get(2)?,
                    question: row.get(3)?,
                    options,
                    context: None,
                    urgency: "medium".to_string(),
                    created_at: row.get(5)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(GetPendingDecisionsOutput { pending_decisions })
    })
}

// ============================================================================
// Helper Functions
// ============================================================================

fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
