//! Coordination tools for task lifecycle management
//!
//! Handles: claim, release, workplan, footprint, heartbeat, stale detection

use crate::db::with_db;
use rusqlite::{params, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default threshold for stale tasks (15 minutes)
const DEFAULT_STALE_THRESHOLD_MINUTES: i64 = 15;

/// God symbol threshold (500 lines)
const GOD_SYMBOL_LINE_LIMIT: i64 = 500;

// ============================================================================
// Input/Output Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimTaskInput {
    pub bead_id: String,
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimTaskOutput {
    pub success: bool,
    pub bead_id: String,
    pub owner: String,
    pub start_hash: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseTaskInput {
    pub bead_id: String,
    pub agent_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReleaseTaskOutput {
    pub success: bool,
    pub bead_id: String,
    pub preserved_context: PreservedContext,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreservedContext {
    pub workplan_summary: Option<String>,
    pub footprint_summary: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Workplan {
    pub modifies: Option<ModifiesSpec>,
    pub creates: Option<CreatesSpec>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModifiesSpec {
    pub files: Option<Vec<String>>,
    pub symbols: Option<Vec<String>>,
    pub modules: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreatesSpec {
    pub symbols: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateWorkplanInput {
    pub bead_id: String,
    pub agent_id: String,
    pub workplan: Workplan,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateWorkplanOutput {
    pub success: bool,
    pub bead_id: String,
    pub start_hash: String,
    pub warnings: Vec<String>,
    pub overlaps: Vec<SymbolOverlap>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SymbolOverlap {
    pub symbol: String,
    pub other_bead_id: String,
    pub other_agent_id: Option<String>,
    pub severity: String,
    pub is_god_symbol: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiffSummary {
    pub files_changed: Vec<String>,
    pub lines_added: Option<i32>,
    pub lines_removed: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BreakingChange {
    pub symbol: String,
    pub change_kind: String,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReportFootprintInput {
    pub bead_id: String,
    pub agent_id: String,
    pub diff_summary: DiffSummary,
    pub breaking_changes: Option<Vec<BreakingChange>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReportFootprintOutput {
    pub success: bool,
    pub bead_id: String,
    pub symbols_touched: Vec<String>,
    pub conflicts: Vec<ConflictInfo>,
    pub notifications_sent: i32,
    pub start_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConflictInfo {
    pub symbol: String,
    pub other_bead_ids: Vec<String>,
    pub suggested_follow_up: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeartbeatInput {
    pub bead_id: String,
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeartbeatOutput {
    pub status: String,
    pub last_heartbeat: i64,
    pub notifications: Vec<NotificationSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationSummary {
    pub id: i64,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub from_agent: Option<String>,
    pub target_symbol: Option<String>,
    pub change_description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListStaleTasksInput {
    pub threshold_minutes: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListStaleTasksOutput {
    pub stale_tasks: Vec<StaleTaskInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StaleTaskInfo {
    pub bead_id: String,
    pub owner: Option<String>,
    pub last_heartbeat: i64,
    pub minutes_stale: i64,
    pub workplan_summary: Option<String>,
}

// ============================================================================
// Tool Implementations
// ============================================================================

/// Claim a task for an agent to work on
pub fn claim_task(input: &ClaimTaskInput) -> Result<ClaimTaskOutput> {
    let now = current_timestamp();

    with_db(|conn| {
        // Check if task already exists
        let existing: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT bead_id, owner FROM tasks WHERE bead_id = ?1",
                [&input.bead_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if let Some((_, Some(owner))) = &existing {
            if owner != &input.agent_id {
                return Ok(ClaimTaskOutput {
                    success: false,
                    bead_id: input.bead_id.clone(),
                    owner: owner.clone(),
                    start_hash: None,
                    message: format!("Task already claimed by {}", owner),
                });
            }
        }

        // Insert or update task
        if existing.is_some() {
            conn.execute(
                "UPDATE tasks SET owner = ?1, last_heartbeat = ?2, last_update = ?3 WHERE bead_id = ?4",
                params![&input.agent_id, now, now, &input.bead_id],
            )?;
        } else {
            conn.execute(
                "INSERT INTO tasks (bead_id, owner, last_heartbeat, last_update) VALUES (?1, ?2, ?3, ?4)",
                params![&input.bead_id, &input.agent_id, now, now],
            )?;
        }

        // Get start_hash if any
        let start_hash: Option<String> = conn
            .query_row(
                "SELECT start_hash FROM tasks WHERE bead_id = ?1",
                [&input.bead_id],
                |row| row.get(0),
            )
            .ok();

        Ok(ClaimTaskOutput {
            success: true,
            bead_id: input.bead_id.clone(),
            owner: input.agent_id.clone(),
            start_hash,
            message: "Task claimed successfully".to_string(),
        })
    })
}

/// Release a task, preserving context for handoff
pub fn release_task(input: &ReleaseTaskInput) -> Result<ReleaseTaskOutput> {
    let now = current_timestamp();

    with_db(|conn| {
        // Get task
        let task: Option<(Option<String>, Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT owner, workplan, footprint FROM tasks WHERE bead_id = ?1",
                [&input.bead_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();

        let Some((owner, workplan, footprint)) = task else {
            return Ok(ReleaseTaskOutput {
                success: false,
                bead_id: input.bead_id.clone(),
                preserved_context: PreservedContext {
                    workplan_summary: None,
                    footprint_summary: None,
                },
                message: "Task not found in coordination tracking".to_string(),
            });
        };

        let owner_str = owner.unwrap_or_default();
        if owner_str != input.agent_id {
            return Ok(ReleaseTaskOutput {
                success: false,
                bead_id: input.bead_id.clone(),
                preserved_context: PreservedContext {
                    workplan_summary: None,
                    footprint_summary: None,
                },
                message: format!("Cannot release - task owned by {}, not {}", owner_str, input.agent_id),
            });
        }

        // Clear owner
        conn.execute(
            "UPDATE tasks SET owner = NULL, last_update = ?1 WHERE bead_id = ?2",
            params![now, &input.bead_id],
        )?;

        // Build summaries
        let workplan_summary = workplan.and_then(|wp: String| {
            serde_json::from_str::<Workplan>(&wp).ok().map(|w| {
                let modifies = w.modifies.as_ref().and_then(|m| m.symbols.as_ref()).map(|s| s.len()).unwrap_or(0);
                let creates = w.creates.as_ref().and_then(|c| c.symbols.as_ref()).map(|s| s.len()).unwrap_or(0);
                format!("{} symbols to modify, {} to create", modifies, creates)
            })
        });

        let footprint_summary = footprint.and_then(|fp: String| {
            serde_json::from_str::<DiffSummary>(&fp).ok().map(|d| {
                format!("{} files changed", d.files_changed.len())
            })
        });

        Ok(ReleaseTaskOutput {
            success: true,
            bead_id: input.bead_id.clone(),
            preserved_context: PreservedContext {
                workplan_summary,
                footprint_summary,
            },
            message: input.reason.clone().unwrap_or_else(|| "Task released successfully".to_string()),
        })
    })
}

/// Update the workplan for a task
pub fn update_workplan(input: &UpdateWorkplanInput, workspace_root: &Path) -> Result<UpdateWorkplanOutput> {
    let now = current_timestamp();

    with_db(|conn| {
        // Ensure task exists
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM tasks WHERE bead_id = ?1",
                [&input.bead_id],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !exists {
            conn.execute(
                "INSERT INTO tasks (bead_id, owner, last_update) VALUES (?1, ?2, ?3)",
                params![&input.bead_id, &input.agent_id, now],
            )?;
        }

        // Store workplan as JSON
        let workplan_json = serde_json::to_string(&input.workplan).unwrap_or_default();

        // Compute start_hash
        let start_hash = compute_start_hash(&input.workplan, workspace_root);

        // Update task
        conn.execute(
            "UPDATE tasks SET workplan = ?1, start_hash = ?2, last_update = ?3 WHERE bead_id = ?4",
            params![workplan_json, start_hash, now, &input.bead_id],
        )?;

        let mut warnings = Vec::new();
        let mut overlaps = Vec::new();

        // Handle modifies
        if let Some(ref modifies) = input.workplan.modifies {
            if let Some(ref symbols) = modifies.symbols {
                for symbol_ref in symbols {
                    // Check if symbol exists
                    let symbol_info: Option<(i64, i64)> = conn
                        .query_row(
                            "SELECT id, line_count FROM symbols WHERE fq_name = ?1",
                            [symbol_ref],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .ok();

                    // Create bead_symbol entry
                    conn.execute(
                        "INSERT OR REPLACE INTO bead_symbols (bead_id, symbol_ref, symbol_id, relation, is_virtual) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            &input.bead_id,
                            symbol_ref,
                            symbol_info.map(|(id, _)| id),
                            "planned-modify",
                            symbol_info.is_none() as i32
                        ],
                    )?;

                    // Check for overlaps
                    let mut stmt = conn.prepare(
                        "SELECT bs.bead_id, t.owner FROM bead_symbols bs JOIN tasks t ON bs.bead_id = t.bead_id WHERE bs.symbol_ref = ?1 AND bs.bead_id != ?2 AND bs.relation IN ('planned-modify', 'actual')"
                    )?;
                    let other_beads: Vec<(String, Option<String>)> = stmt
                        .query_map(params![symbol_ref, &input.bead_id], |row| {
                            Ok((row.get(0)?, row.get(1)?))
                        })?
                        .filter_map(|r| r.ok())
                        .collect();

                    if !other_beads.is_empty() {
                        let is_god_symbol = symbol_info.map(|(_, lc)| lc > GOD_SYMBOL_LINE_LIMIT).unwrap_or(false);

                        for (other_bead_id, other_owner) in &other_beads {
                            let severity = if is_god_symbol {
                                "low"
                            } else if other_beads.len() > 1 {
                                "high"
                            } else {
                                "medium"
                            };

                            overlaps.push(SymbolOverlap {
                                symbol: symbol_ref.clone(),
                                other_bead_id: other_bead_id.clone(),
                                other_agent_id: other_owner.clone(),
                                severity: severity.to_string(),
                                is_god_symbol,
                            });
                        }

                        if is_god_symbol {
                            warnings.push(format!(
                                "{} is a god-symbol ({} lines) - overlap treated as low severity",
                                symbol_ref,
                                symbol_info.unwrap().1
                            ));
                        }
                    }
                }
            }
        }

        // Handle creates
        if let Some(ref creates) = input.workplan.creates {
            if let Some(ref symbols) = creates.symbols {
                for symbol_ref in symbols {
                    conn.execute(
                        "INSERT OR REPLACE INTO bead_symbols (bead_id, symbol_ref, symbol_id, relation, is_virtual) VALUES (?1, ?2, NULL, 'planned-create', 1)",
                        params![&input.bead_id, symbol_ref],
                    )?;
                }
            }
        }

        Ok(UpdateWorkplanOutput {
            success: true,
            bead_id: input.bead_id.clone(),
            start_hash,
            warnings,
            overlaps,
        })
    })
}

/// Send heartbeat to keep task alive and get notifications
pub fn heartbeat(input: &HeartbeatInput) -> Result<HeartbeatOutput> {
    let now = current_timestamp();

    with_db(|conn| {
        // Update heartbeat
        let changes = conn.execute(
            "UPDATE tasks SET last_heartbeat = ?1 WHERE bead_id = ?2 AND owner = ?3",
            params![now, &input.bead_id, &input.agent_id],
        )?;

        if changes == 0 {
            return Ok(HeartbeatOutput {
                status: "error".to_string(),
                last_heartbeat: now,
                notifications: vec![],
            });
        }

        // Get pending notifications
        let mut stmt = conn.prepare(
            "SELECT id, notification_type, from_agent, target_symbol, change_description FROM notifications WHERE target_agent = ?1 AND status = 'pending' ORDER BY created_at DESC LIMIT 10"
        )?;
        let notifications: Vec<NotificationSummary> = stmt
            .query_map([&input.agent_id], |row| {
                Ok(NotificationSummary {
                    id: row.get(0)?,
                    notification_type: row.get(1)?,
                    from_agent: row.get(2)?,
                    target_symbol: row.get(3)?,
                    change_description: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(HeartbeatOutput {
            status: "ok".to_string(),
            last_heartbeat: now,
            notifications,
        })
    })
}

/// List tasks that haven't had a heartbeat recently
pub fn list_stale_tasks(input: &ListStaleTasksInput) -> Result<ListStaleTasksOutput> {
    let threshold_minutes = input.threshold_minutes.unwrap_or(DEFAULT_STALE_THRESHOLD_MINUTES);
    let threshold_ms = threshold_minutes * 60 * 1000;
    let now = current_timestamp();
    let cutoff = now - threshold_ms;

    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT bead_id, owner, last_heartbeat, workplan FROM tasks WHERE owner IS NOT NULL AND last_heartbeat IS NOT NULL AND last_heartbeat < ?1"
        )?;

        let stale_tasks: Vec<StaleTaskInfo> = stmt
            .query_map([cutoff], |row| {
                let bead_id: String = row.get(0)?;
                let owner: Option<String> = row.get(1)?;
                let last_heartbeat: i64 = row.get(2)?;
                let workplan: Option<String> = row.get(3)?;

                let workplan_summary = workplan.and_then(|wp: String| {
                    serde_json::from_str::<Workplan>(&wp).ok().and_then(|w| {
                        w.modifies.and_then(|m| {
                            m.symbols.map(|s| {
                                if s.is_empty() {
                                    "No symbols specified".to_string()
                                } else {
                                    format!("Modifying {}", s.join(", "))
                                }
                            })
                        })
                    })
                });

                Ok(StaleTaskInfo {
                    bead_id,
                    owner,
                    last_heartbeat,
                    minutes_stale: (now - last_heartbeat) / 60000,
                    workplan_summary,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(ListStaleTasksOutput { stale_tasks })
    })
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get current timestamp in milliseconds
fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Compute a hash of the relevant files for drift detection
fn compute_start_hash(workplan: &Workplan, workspace_root: &Path) -> String {
    let mut hasher = Sha256::new();
    let mut files = Vec::new();

    if let Some(ref modifies) = workplan.modifies {
        if let Some(ref fs) = modifies.files {
            files.extend(fs.iter().cloned());
        }
    }

    files.sort();

    for file in &files {
        let file_path = workspace_root.join(file);
        if let Ok(content) = fs::read(&file_path) {
            hasher.update(file.as_bytes());
            hasher.update(&content);
        }
    }

    if files.is_empty() {
        hasher.update(b"empty");
    }

    let result = hasher.finalize();
    hex::encode(&result[..8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use tempfile::tempdir;

    fn setup_test_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap()), true).unwrap();
    }

    #[test]
    fn test_claim_task() {
        setup_test_db();

        let input = ClaimTaskInput {
            bead_id: "TEST-1".to_string(),
            agent_id: "agent-a".to_string(),
        };

        let result = claim_task(&input).unwrap();
        assert!(result.success);
        assert_eq!(result.owner, "agent-a");
    }

    #[test]
    fn test_heartbeat() {
        setup_test_db();

        // First claim a task
        let claim = ClaimTaskInput {
            bead_id: "TEST-2".to_string(),
            agent_id: "agent-b".to_string(),
        };
        claim_task(&claim).unwrap();

        // Then heartbeat
        let input = HeartbeatInput {
            bead_id: "TEST-2".to_string(),
            agent_id: "agent-b".to_string(),
        };

        let result = heartbeat(&input).unwrap();
        assert_eq!(result.status, "ok");
    }
}
