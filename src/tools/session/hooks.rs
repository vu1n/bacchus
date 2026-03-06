//! Hook checks: session blocking decisions for stop hooks.

use crate::config::find_workspace_root;
use crate::db::with_db;
use crate::messages;
use crate::tasks;
use crate::tools::orchestrator;
use crate::workers;

use std::path::Path;

use super::config::*;
use super::file::*;
use super::heartbeat::attach_agent_session_heartbeat;
use super::lifecycle::stop_session;
use super::types::{HookCheckOutput, Session, SessionMode};
use super::workers as session_workers;

// ============================================================================
// Circuit breaker: rapid-block detection for agent stop hooks
// ============================================================================

/// Window (ms) within which consecutive blocks are considered "rapid".
const RAPID_BLOCK_WINDOW_MS: i64 = 2000;
/// After this many rapid blocks, emit a softer "you may be rate-limited" message.
const RAPID_BLOCK_SOFT_THRESHOLD: u32 = 5;
/// After this many rapid blocks, approve exit to break the feedback loop.
const RAPID_BLOCK_HARD_THRESHOLD: u32 = 20;

/// Read the block counter file. Returns (count, last_timestamp_ms).
fn read_block_counter(path: &Path) -> (u32, i64) {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let mut parts = content.trim().splitn(2, ' ');
            let count = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let ts = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            (count, ts)
        }
        Err(_) => (0, 0),
    }
}

/// Write block counter state.
fn write_block_counter(path: &Path, count: u32, ts: i64) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format!("{} {}", count, ts));
}

/// Get the block counter file path for the current session scope.
fn block_counter_path() -> Option<std::path::PathBuf> {
    sessions_dir().map(|dir| {
        let scope = crate::config::current_session_scope_id();
        dir.join(format!("{}_block_count", crate::config::sanitize_scope(&scope)))
    })
}

/// Check if session should block exit (for stop hook)
pub fn check_session() -> HookCheckOutput {
    // Read session file
    let session = match session_path() {
        Some(path) if path.exists() => match read_session(&path) {
            Ok(s) => s,
            Err(_) => {
                return HookCheckOutput {
                    decision: "approve".to_string(),
                    reason: "Invalid session file".to_string(),
                }
            }
        },
        _ => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "No bacchus session active".to_string(),
            }
        }
    };

    match session.mode {
        SessionMode::Agent => check_agent_session(&session),
        SessionMode::Orchestrator => check_orchestrator_session(&session),
        SessionMode::Architect => check_architect_session(&session),
    }
}

pub(super) fn check_agent_session(session: &Session) -> HookCheckOutput {
    let task_id = match &session.task_id {
        Some(id) => id,
        None => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "No task ID in session".to_string(),
            }
        }
    };

    // Get workspace root for task lookup
    let _workspace_root = match find_workspace_root() {
        Some(root) => root,
        None => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "Cannot find workspace root".to_string(),
            }
        }
    };

    // Check task status
    match tasks::get_sqlite_task(task_id) {
        Ok(task) => {
            if let Some(owner) = task.claimed_by.as_deref() {
                if let Some(session_agent_id) = session.agent_id.as_deref() {
                    if owner != session_agent_id {
                        let _ = stop_session();
                        return HookCheckOutput {
                            decision: "approve".to_string(),
                            reason: format!(
                                "Task {} is now owned by {} (session owner was {}). Session cleared.",
                                task_id, owner, session_agent_id
                            ),
                        };
                    }
                } else {
                    let _ = attach_agent_session_heartbeat(task_id, owner);
                }
            }

            match task.status {
                tasks::SqliteTaskStatus::Closed => {
                    let _ = stop_session();
                    HookCheckOutput {
                        decision: "approve".to_string(),
                        reason: format!("Task {} is closed. Session cleared.", task_id),
                    }
                }
                tasks::SqliteTaskStatus::Blocked => {
                    let _ = stop_session();
                    HookCheckOutput {
                        decision: "approve".to_string(),
                        reason: format!("Task {} is blocked. Session cleared.", task_id),
                    }
                }
                tasks::SqliteTaskStatus::InProgress => {
                    if let Some(session_agent_id) = session.agent_id.as_deref() {
                        if task.claimed_by.as_deref() == Some(session_agent_id) {
                            let _ = tasks::heartbeat_sqlite_task(task_id, session_agent_id);
                        }
                    }

                    // Circuit breaker: detect rapid-fire blocks (rate limit loops)
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    if let Some(counter_path) = block_counter_path() {
                        let (count, last_ts) = read_block_counter(&counter_path);
                        let is_rapid = (now_ms - last_ts) < RAPID_BLOCK_WINDOW_MS;

                        if is_rapid && count >= RAPID_BLOCK_HARD_THRESHOLD {
                            // Probable rate limit loop — let agent exit to break the cycle
                            write_block_counter(&counter_path, 0, now_ms);
                            return HookCheckOutput {
                                decision: "approve".to_string(),
                                reason: format!(
                                    "Task {} is in_progress but agent appears rate-limited ({} rapid blocks). Allowing exit to break feedback loop.",
                                    task_id, count
                                ),
                            };
                        }

                        if is_rapid {
                            write_block_counter(&counter_path, count + 1, now_ms);
                        } else {
                            // Normal cadence — reset counter
                            write_block_counter(&counter_path, 1, now_ms);
                        }

                        if is_rapid && count >= RAPID_BLOCK_SOFT_THRESHOLD {
                            return HookCheckOutput {
                                decision: "block".to_string(),
                                reason: format!(
                                    "Task {} is in_progress. You may be rate-limited — pause briefly before retrying. If stuck, run 'bacchus release {} --status blocked'.",
                                    task_id, task_id
                                ),
                            };
                        }
                    }

                    HookCheckOutput {
                        decision: "block".to_string(),
                        reason: format!(
                            "Task {} is in_progress. Continue working, then run 'bacchus release {} --status done' or '--status blocked'.",
                            task_id, task_id
                        ),
                    }
                }
                tasks::SqliteTaskStatus::ReadyForRelease => HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} is ready_for_release. Wait for orchestrator merge, or run 'bacchus process-releases' from an orchestrator flow.",
                        task_id
                    ),
                },
                tasks::SqliteTaskStatus::Releasing => HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} is currently releasing. Wait for merge/conflict outcome before exiting.",
                        task_id
                    ),
                },
                tasks::SqliteTaskStatus::NeedsResolution => HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} needs conflict resolution. Resolve conflicts, then run 'bacchus resolve {}' (or 'bacchus abort {}' to continue editing).",
                        task_id, task_id, task_id
                    ),
                },
                tasks::SqliteTaskStatus::Open | tasks::SqliteTaskStatus::Draft => HookCheckOutput {
                    decision: "block".to_string(),
                    reason: format!(
                        "Task {} is '{}'. Reclaim it with 'bacchus claim {} --agent-id <agent_id>' or stop the session if this task is no longer assigned.",
                        task_id,
                        task.status.as_str(),
                        task_id
                    ),
                },
            }
        }
        Err(e) => HookCheckOutput {
            decision: "approve".to_string(),
            reason: format!("Cannot check task status: {}", e),
        },
    }
}

pub(super) fn check_orchestrator_session(session: &Session) -> HookCheckOutput {
    let max_concurrent = session.max_concurrent.unwrap_or(3);
    let now = chrono::Utc::now().timestamp_millis();
    let active_cutoff = now - tasks::CLAIM_HEARTBEAT_TIMEOUT_MS;
    let run_id = session
        .run_id
        .as_deref()
        .unwrap_or(session.started_at.as_str());

    match tasks::try_acquire_orchestrator_lease(run_id, configured_orchestrator_lease_ttl_ms()) {
        Ok(true) => {}
        Ok(false) => {
            let details = session_workers::describe_existing_orchestrator_lease()
                .unwrap_or_else(|| "holder=unknown".to_string());
            let _ = stop_session();
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: format!(
                    "Another orchestrator leader lease is active ({}). Session cleared.",
                    details
                ),
            };
        }
        Err(e) => {
            return HookCheckOutput {
                decision: "block".to_string(),
                reason: format!("Failed to renew orchestrator lease: {}", e),
            };
        }
    }

    // Get workspace root for task lookup and config loading
    let workspace_root = match find_workspace_root() {
        Some(root) => root,
        None => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "Cannot find workspace root".to_string(),
            }
        }
    };
    let wcfg = resolve_worker_config(Some(&workspace_root));
    let worker_stale_cutoff = active_cutoff - wcfg.stale_grace_ms;

    let cycle = session_workers::run_recovery_cycle(
        &workspace_root,
        run_id,
        now,
        worker_stale_cutoff,
        wcfg.max_runtime_ms,
        wcfg.kill_stale,
    );
    let recovery_note = cycle.recovery_note;
    let failed_reconcile_note = cycle.reconcile_note;

    // Best-effort release processing: integrates completed agent work before scheduling more.
    let release_note =
        match orchestrator::process_ready_releases(&workspace_root, Some(20), Some(run_id)) {
            Ok(summary) if summary.processed > 0 => Some(format!(
                "Processed releases: reconciled {}, merged {}, conflicts {}, failed {}.",
                summary.reconciled, summary.merged, summary.conflicts, summary.failed
            )),
            Ok(summary) if summary.reconciled > 0 => Some(format!(
                "Processed releases: reconciled {}, merged {}, conflicts {}, failed {}.",
                summary.reconciled, summary.merged, summary.conflicts, summary.failed
            )),
            Ok(_) => None,
            Err(e) => Some(format!("Release processing error: {}.", e)),
        };
    let combined_note = {
        let mut notes = Vec::new();
        if let Some(n) = release_note {
            notes.push(n);
        }
        if let Some(n) = recovery_note {
            notes.push(n);
        }
        if let Some(n) = failed_reconcile_note {
            notes.push(n);
        }
        if notes.is_empty() {
            None
        } else {
            Some(notes.join(" "))
        }
    };

    // Get project stats
    let ready_tasks = tasks::get_ready_sqlite_tasks(None).unwrap_or_default();
    let ready_count = ready_tasks.len();
    let ready_for_release = tasks::get_tasks_ready_for_release().unwrap_or_default();
    let ready_for_release_count = ready_for_release.len();

    // Active claims are heartbeat-fresh claims only.
    let active_count = session_workers::count_active_claims(active_cutoff);
    let active_workers = workers::count_active_workers(run_id).unwrap_or(0);

    // Tasks in_progress without claim owner should still block orchestration.
    let orphaned_in_progress_ids: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM tasks
             WHERE status = 'in_progress'
               AND claimed_by IS NULL
               AND deleted_at IS NULL
             ORDER BY priority, created_at",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let ids = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    })
    .unwrap_or_default();
    let orphaned_in_progress_count = orphaned_in_progress_ids.len();

    // Stale claimed tasks should not consume orchestrator capacity.
    let stale_claim_ids: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM tasks
             WHERE status = 'in_progress'
               AND claimed_by IS NOT NULL
               AND COALESCE(claimed_heartbeat_at, claimed_at, 0) < ?1
               AND deleted_at IS NULL
             ORDER BY priority, created_at",
        )?;
        let rows = stmt.query_map([active_cutoff], |row| row.get(0))?;
        let ids = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    })
    .unwrap_or_default();
    let stale_claim_count = stale_claim_ids.len();

    if ready_for_release_count > 0 {
        let task_ids: Vec<_> = ready_for_release.iter().map(|t| t.id.as_str()).collect();
        let mut reason = format!(
            "{} task(s) still ready_for_release after automatic release attempt: {}. Review failures/conflicts and rerun 'bacchus process-releases' if needed.",
            ready_for_release_count,
            task_ids.join(", ")
        );
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if orphaned_in_progress_count > 0 {
        let mut reason = format!(
            "{} task(s) are in_progress without claims: {}. Reclaim with 'bacchus claim <id> --agent-id <agent> --force' or reset status.",
            orphaned_in_progress_count,
            orphaned_in_progress_ids.join(", ")
        );
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if ready_count > 0 && active_count < max_concurrent as usize {
        // Ready work available and capacity to spawn.
        let slots = max_concurrent as usize - active_count;
        let to_spawn = ready_count.min(slots);
        let task_ids: Vec<_> = ready_tasks
            .iter()
            .take(to_spawn)
            .map(|t| t.id.as_str())
            .collect();

        let mut reason = if configured_orchestrator_auto_spawn(&wcfg) {
            if let Some(ref worker_cmd) = wcfg.cmd {
                let summary = session_workers::try_spawn_workers(
                    &workspace_root,
                    run_id,
                    &ready_tasks,
                    slots,
                    worker_cmd,
                    &wcfg,
                );
                let post_active_count = session_workers::count_active_claims(active_cutoff);
                let post_active_workers = workers::count_active_workers(run_id).unwrap_or(0);
                let post_ready_count = tasks::get_ready_sqlite_tasks(None)
                    .unwrap_or_default()
                    .len();

                let mut msg = format!(
                    "Auto-spawn attempted {} task(s): launched {}, failed {}, backoff {}, exhausted {}. Ready remaining: {}. Active claims: {}/{}. Active workers: {}.",
                    summary.attempted,
                    summary.launched,
                    summary.failed,
                    summary.skipped_backoff,
                    summary.exhausted,
                    post_ready_count,
                    post_active_count,
                    max_concurrent,
                    post_active_workers
                );
                if !summary.errors.is_empty() {
                    msg = format!("{} Errors: {}", msg, summary.errors.join(" | "));
                }
                if summary.launched == 0 && summary.failed == 0 && summary.skipped_backoff == 0 {
                    msg = format!(
                        "{} No workers launched for candidates: {}.",
                        msg,
                        task_ids.join(", ")
                    );
                }
                msg
            } else {
                format!(
                    "Ready to spawn {} agent(s) for: {}. Active: {}/{}. Auto-spawn enabled but worker.cmd is not configured. Set worker.cmd in .bacchus/config.yaml (or BACCHUS_WORKER_CMD env var), then run 'bacchus session spawn-workers --count {}'.",
                    to_spawn,
                    task_ids.join(", "),
                    active_count,
                    max_concurrent,
                    to_spawn
                )
            }
        } else {
            format!(
                "Ready to spawn {} agent(s) for: {}. Active: {}/{}. Use 'bacchus session spawn-workers --count {}' (with worker.cmd in .bacchus/config.yaml or BACCHUS_WORKER_CMD env var) or 'bacchus claim <task_id> --agent-id <agent_id>'.",
                to_spawn,
                task_ids.join(", "),
                active_count,
                max_concurrent,
                to_spawn
            )
        };

        if stale_claim_count > 0 {
            reason = format!(
                "{} Ignoring {} stale claim(s): {}. Run 'bacchus stale --cleanup' to reclaim them.",
                reason,
                stale_claim_count,
                stale_claim_ids.join(", ")
            );
        }
        if active_workers > 0 {
            reason = format!(
                "{} Worker processes currently active: {}.",
                reason, active_workers
            );
        }
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if active_count > 0 {
        // Active claims - wait for agents to complete
        let mut reason = format!(
            "Waiting for {} active agent(s) to complete. Check with 'bacchus list'.",
            active_count
        );
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if stale_claim_count > 0 {
        // No active claims but stale claimed tasks remain.
        let mut reason = format!(
            "{} stale claim(s) detected: {}. Run 'bacchus stale --cleanup' or reclaim manually.",
            stale_claim_count,
            stale_claim_ids.join(", ")
        );
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if ready_count == 0 && ready_for_release_count == 0 {
        // No ready, no in_progress, no claims - all done or all blocked
        let _ = stop_session();
        let mut reason = "All work complete or blocked. Session cleared.".to_string();
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }

        HookCheckOutput {
            decision: "approve".to_string(),
            reason,
        }
    } else {
        let mut reason = "Orchestrator complete".to_string();
        if let Some(note) = &combined_note {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "approve".to_string(),
            reason,
        }
    }
}

pub(super) fn check_architect_session(session: &Session) -> HookCheckOutput {
    let agent_id = match &session.agent_id {
        Some(id) => id,
        None => {
            return HookCheckOutput {
                decision: "approve".to_string(),
                reason: "No agent ID in architect session".to_string(),
            }
        }
    };

    // Best-effort stale message recovery so architect sessions don't deadlock on abandoned locks.
    let reclaim_note = match messages::reclaim_stale_messages() {
        Ok((requeued, failed)) if requeued > 0 || failed > 0 => Some(format!(
            "Recovered stale messages (requeued: {}, failed: {}).",
            requeued, failed
        )),
        _ => None,
    };

    // Check for pending messages for this architect
    let pending_count = with_db(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM agent_messages WHERE target_agent = ?1 AND status = 'pending'",
            [agent_id.as_str()],
            |r| r.get::<_, i32>(0),
        )
    })
    .unwrap_or(0);

    // Check for messages currently being processed
    let processing_count = with_db(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM agent_messages WHERE processing_by = ?1 AND status = 'processing'",
            [agent_id.as_str()],
            |r| r.get::<_, i32>(0),
        )
    })
    .unwrap_or(0);

    // Check for epics in planning state (architect's responsibility)
    let planning_epics = with_db(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM epics WHERE status = 'planning'",
            [],
            |r| r.get::<_, i32>(0),
        )
    })
    .unwrap_or(0);

    if processing_count > 0 {
        let mut reason = format!(
            "Architect {} has {} message(s) being processed. Complete processing before exiting.",
            agent_id, processing_count
        );
        if let Some(note) = reclaim_note.as_deref() {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if pending_count > 0 {
        let mut reason = format!(
            "Architect {} has {} pending message(s). Run 'bacchus message claim {} --limit 10', process each item, then ack with 'bacchus message ack <message_id> {}'.",
            agent_id, pending_count, agent_id, agent_id
        );
        if let Some(note) = reclaim_note.as_deref() {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else if planning_epics > 0 {
        let mut reason = format!(
            "{} epic(s) in 'planning' state. Break down into tasks, then set status with 'bacchus epic set-status <epic_id> active'.",
            planning_epics
        );
        if let Some(note) = reclaim_note.as_deref() {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "block".to_string(),
            reason,
        }
    } else {
        // No pending work - architect can exit
        let _ = stop_session();
        let mut reason = "No pending work for architect. Session cleared.".to_string();
        if let Some(note) = reclaim_note.as_deref() {
            reason = format!("{} {}", note, reason);
        }
        HookCheckOutput {
            decision: "approve".to_string(),
            reason,
        }
    }
}
