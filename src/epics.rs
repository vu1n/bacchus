//! Epic management module
//!
//! Epics are high-level work containers created by humans or architect agents.
//! Tasks belong to epics and cannot exist without one.

use crate::db::{with_db, with_db_typed};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use thiserror::Error;

// ============================================================================
// Types
// ============================================================================

/// Epic status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpicStatus {
    Open,
    Planning,
    Active,
    Closed,
}

impl EpicStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            EpicStatus::Open => "open",
            EpicStatus::Planning => "planning",
            EpicStatus::Active => "active",
            EpicStatus::Closed => "closed",
        }
    }
}

impl std::str::FromStr for EpicStatus {
    type Err = EpicsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(EpicStatus::Open),
            "planning" => Ok(EpicStatus::Planning),
            "active" => Ok(EpicStatus::Active),
            "closed" => Ok(EpicStatus::Closed),
            _ => Err(EpicsError::InvalidStatus(s.to_string())),
        }
    }
}

impl std::fmt::Display for EpicStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// An epic - a high-level work container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Epic {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: EpicStatus,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Input for creating a new epic
#[derive(Debug, Clone)]
pub struct CreateEpicInput {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub created_by: String,
}

/// Errors that can occur when working with epics
#[derive(Debug, Error)]
pub enum EpicsError {
    #[error("Epic not found: {0}")]
    NotFound(String),

    #[error("Epic already exists: {0}")]
    DuplicateEpic(String),

    #[error("Invalid status: {0}")]
    InvalidStatus(String),

    #[error("Invalid status transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },

    #[error("Database error: {0}")]
    DbError(String),
}

impl From<rusqlite::Error> for EpicsError {
    fn from(e: rusqlite::Error) -> Self {
        EpicsError::DbError(e.to_string())
    }
}

// ============================================================================
// Epic CRUD Operations
// ============================================================================

/// Create a new epic
pub fn create_epic(input: CreateEpicInput) -> Result<Epic, EpicsError> {
    let now = chrono::Utc::now().timestamp_millis();

    // Check for duplicate first
    let exists: bool = with_db(|conn| {
        conn.query_row("SELECT 1 FROM epics WHERE id = ?1", [&input.id], |_| {
            Ok(true)
        })
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            _ => Err(e),
        })
    })
    .map_err(|e: rusqlite::Error| EpicsError::DbError(e.to_string()))?;

    if exists {
        return Err(EpicsError::DuplicateEpic(input.id.clone()));
    }

    with_db(|conn| {
        conn.execute(
            "INSERT INTO epics (id, title, description, status, created_by, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?5)",
            params![
                input.id,
                input.title,
                input.description,
                input.created_by,
                now
            ],
        )
    })
    .map_err(|e: rusqlite::Error| EpicsError::DbError(e.to_string()))?;

    Ok(Epic {
        id: input.id,
        title: input.title,
        description: input.description,
        status: EpicStatus::Open,
        created_by: input.created_by,
        created_at: now,
        updated_at: now,
    })
}

/// Get an epic by ID
pub fn get_epic(epic_id: &str) -> Result<Epic, EpicsError> {
    with_db(|conn| {
        conn.query_row(
            "SELECT id, title, description, status, created_by, created_at, updated_at
             FROM epics WHERE id = ?1",
            [epic_id],
            |row| {
                let status_str: String = row.get(3)?;
                Ok(Epic {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    description: row.get(2)?,
                    status: EpicStatus::from_str(&status_str).unwrap_or(EpicStatus::Open),
                    created_by: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
    })
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => EpicsError::NotFound(epic_id.to_string()),
        e => EpicsError::DbError(e.to_string()),
    })
}

/// List epics with optional status filter
pub fn list_epics(status: Option<EpicStatus>) -> Result<Vec<Epic>, EpicsError> {
    with_db(|conn| {
        let status_str = status.map(|s| s.as_str().to_string());

        let sql = if status_str.is_some() {
            "SELECT id, title, description, status, created_by, created_at, updated_at
             FROM epics WHERE status = ?1 ORDER BY created_at DESC"
        } else {
            "SELECT id, title, description, status, created_by, created_at, updated_at
             FROM epics ORDER BY created_at DESC"
        };

        let mut stmt = conn.prepare(sql)?;
        let row_mapper = |row: &rusqlite::Row| {
            let status_str: String = row.get(3)?;
            Ok(Epic {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                status: EpicStatus::from_str(&status_str).unwrap_or(EpicStatus::Open),
                created_by: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        };

        let epics: Vec<Epic> = if let Some(ref s) = status_str {
            stmt.query_map([s.as_str()], row_mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map([], row_mapper)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        Ok(epics)
    })
    .map_err(|e: rusqlite::Error| EpicsError::DbError(e.to_string()))
}

/// Assign an epic to an architect agent for breakdown
///
/// This atomically:
/// 1. Updates the epic status to 'planning'
/// 2. Sends an 'epic_assigned' message to the architect
///
/// Returns an error if the epic is not in 'open' status.
pub fn assign_epic(epic_id: &str, architect_agent: &str) -> Result<Epic, EpicsError> {
    let now = chrono::Utc::now().timestamp_millis();

    with_db_typed(|conn| {
        crate::db::with_savepoint(conn, "assign_epic", || {
            let affected = conn.execute(
                "UPDATE epics SET status = 'planning', updated_at = ?1 WHERE id = ?2 AND status = 'open'",
                params![now, epic_id],
            )?;

            if affected == 0 {
                let exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM epics WHERE id = ?1",
                        [epic_id],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);

                if !exists {
                    return Err(EpicsError::NotFound(epic_id.to_string()));
                } else {
                    return Err(EpicsError::InvalidTransition {
                        from: "non-open".to_string(),
                        to: "planning".to_string(),
                    });
                }
            }

            let payload = serde_json::json!({
                "epic_id": epic_id,
                "assigned_at": now,
            })
            .to_string();

            conn.execute(
                "INSERT INTO agent_messages (target_agent, message_type, payload, status, created_at)
                 VALUES (?1, 'epic_assigned', ?2, 'pending', ?3)",
                params![architect_agent, payload, now],
            )?;

            Ok(conn.query_row(
                "SELECT id, title, description, status, created_by, created_at, updated_at
                 FROM epics WHERE id = ?1",
                [epic_id],
                |row| {
                    let status_str: String = row.get(3)?;
                    Ok(Epic {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        status: EpicStatus::from_str(&status_str).unwrap_or(EpicStatus::Planning),
                        created_by: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )?)
        })
    })
}

fn is_valid_status_transition(from: EpicStatus, to: EpicStatus) -> bool {
    use EpicStatus::*;
    match from {
        Open => matches!(to, Planning | Closed),
        Planning => matches!(to, Open | Active | Closed),
        Active => matches!(to, Open | Closed),
        Closed => matches!(to, Open | Active),
    }
}

/// Update an epic status with transition validation.
pub fn update_epic_status(epic_id: &str, status: EpicStatus) -> Result<Epic, EpicsError> {
    let now = chrono::Utc::now().timestamp_millis();
    let existing = get_epic(epic_id)?;
    let current = existing.status;

    if current == status {
        return Ok(existing);
    }
    if !is_valid_status_transition(current, status) {
        return Err(EpicsError::InvalidTransition {
            from: current.as_str().to_string(),
            to: status.as_str().to_string(),
        });
    }

    with_db_typed(|conn| {
        let affected = conn.execute(
            "UPDATE epics SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now, epic_id],
        )?;
        if affected == 0 {
            return Err(EpicsError::NotFound(epic_id.to_string()));
        }
        Ok(())
    })?;

    get_epic(epic_id)
}

/// Get epic with task counts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpicWithCounts {
    #[serde(flatten)]
    pub epic: Epic,
    pub task_counts: TaskCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCounts {
    pub total: i32,
    pub draft: i32,
    pub open: i32,
    pub in_progress: i32,
    pub blocked: i32,
    pub closed: i32,
}

/// Get an epic with its task counts
pub fn get_epic_with_counts(epic_id: &str) -> Result<EpicWithCounts, EpicsError> {
    let epic = get_epic(epic_id)?;

    let counts = with_db(|conn| {
        let counts: TaskCounts = conn.query_row(
            "SELECT
                COUNT(*) as total,
                SUM(CASE WHEN status = 'draft' THEN 1 ELSE 0 END) as draft,
                SUM(CASE WHEN status = 'open' THEN 1 ELSE 0 END) as open,
                SUM(CASE WHEN status = 'in_progress' THEN 1 ELSE 0 END) as in_progress,
                SUM(CASE WHEN status = 'blocked' THEN 1 ELSE 0 END) as blocked,
                SUM(CASE WHEN status = 'closed' THEN 1 ELSE 0 END) as closed
             FROM tasks WHERE epic_id = ?1 AND deleted_at IS NULL",
            [epic_id],
            |row| {
                Ok(TaskCounts {
                    total: row.get(0)?,
                    draft: row.get(1)?,
                    open: row.get(2)?,
                    in_progress: row.get(3)?,
                    blocked: row.get(4)?,
                    closed: row.get(5)?,
                })
            },
        )?;
        Ok(counts)
    })
    .map_err(|e: rusqlite::Error| EpicsError::DbError(e.to_string()))?;

    Ok(EpicWithCounts {
        epic,
        task_counts: counts,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::setup_empty_test_db;

    #[test]
    fn test_create_epic() {
        let _dir = setup_empty_test_db();

        let input = CreateEpicInput {
            id: "EPIC-001".to_string(),
            title: "Test Epic".to_string(),
            description: Some("A test epic".to_string()),
            created_by: "human".to_string(),
        };

        let epic = create_epic(input).unwrap();
        assert_eq!(epic.id, "EPIC-001");
        assert_eq!(epic.status, EpicStatus::Open);

        crate::db::close_db();
    }

    #[test]
    fn test_get_epic() {
        let _dir = setup_empty_test_db();

        let input = CreateEpicInput {
            id: "EPIC-002".to_string(),
            title: "Test Epic 2".to_string(),
            description: None,
            created_by: "human".to_string(),
        };

        create_epic(input).unwrap();
        let epic = get_epic("EPIC-002").unwrap();
        assert_eq!(epic.title, "Test Epic 2");
        assert!(epic.description.is_none());

        crate::db::close_db();
    }

    #[test]
    fn test_list_epics() {
        let _dir = setup_empty_test_db();

        for i in 1..=3 {
            let input = CreateEpicInput {
                id: format!("EPIC-{:03}", i),
                title: format!("Epic {}", i),
                description: None,
                created_by: "human".to_string(),
            };
            create_epic(input).unwrap();
        }

        let all_epics = list_epics(None).unwrap();
        assert_eq!(all_epics.len(), 3);

        let open_epics = list_epics(Some(EpicStatus::Open)).unwrap();
        assert_eq!(open_epics.len(), 3);

        crate::db::close_db();
    }

    #[test]
    fn test_duplicate_epic_error() {
        let _dir = setup_empty_test_db();

        let input = CreateEpicInput {
            id: "EPIC-DUP".to_string(),
            title: "Test Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        };

        create_epic(input.clone()).unwrap();
        let result = create_epic(input);
        assert!(matches!(result, Err(EpicsError::DuplicateEpic(_))));

        crate::db::close_db();
    }

    #[test]
    fn test_epic_not_found_error() {
        let _dir = setup_empty_test_db();

        let result = get_epic("NONEXISTENT");
        assert!(matches!(result, Err(EpicsError::NotFound(_))));

        crate::db::close_db();
    }

    #[test]
    fn test_update_epic_status_valid_transition() {
        let _dir = setup_empty_test_db();

        let input = CreateEpicInput {
            id: "EPIC-STATUS".to_string(),
            title: "Status Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        };
        create_epic(input).unwrap();

        let planning = update_epic_status("EPIC-STATUS", EpicStatus::Planning).unwrap();
        assert_eq!(planning.status, EpicStatus::Planning);

        let active = update_epic_status("EPIC-STATUS", EpicStatus::Active).unwrap();
        assert_eq!(active.status, EpicStatus::Active);

        crate::db::close_db();
    }

    #[test]
    fn test_update_epic_status_invalid_transition() {
        let _dir = setup_empty_test_db();

        let input = CreateEpicInput {
            id: "EPIC-BAD-STATUS".to_string(),
            title: "Status Epic".to_string(),
            description: None,
            created_by: "human".to_string(),
        };
        create_epic(input).unwrap();

        let result = update_epic_status("EPIC-BAD-STATUS", EpicStatus::Active);
        assert!(matches!(result, Err(EpicsError::InvalidTransition { .. })));

        crate::db::close_db();
    }
}
