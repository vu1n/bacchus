//! Orchestrator lease management.

use rusqlite::params;

use crate::db::with_db;

use super::types::*;

/// In-progress claims with heartbeat older than this are treated as stale for scheduling.
pub const CLAIM_HEARTBEAT_TIMEOUT_MS: i64 = 15 * 60 * 1000;
/// Default leader lease TTL for orchestrator sessions.
pub const ORCHESTRATOR_LEASE_TTL_MS: i64 = 90 * 1000;
pub(crate) const ORCHESTRATOR_LEASE_NAME: &str = "global";

/// Try to acquire (or renew) the global orchestrator leader lease.
///
/// Returns `Ok(true)` when the caller now holds the lease, `Ok(false)` if a different
/// holder still has a non-expired lease.
pub fn try_acquire_orchestrator_lease(holder_id: &str, ttl_ms: i64) -> Result<bool, TasksError> {
    let now = chrono::Utc::now().timestamp_millis();
    let expires_at = now + ttl_ms.max(1);

    with_db(|conn| {
        let affected = conn.execute(
            "INSERT INTO orchestrator_leases (lease_name, holder_id, lease_expires_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(lease_name) DO UPDATE SET
                holder_id = excluded.holder_id,
                lease_expires_at = excluded.lease_expires_at,
                updated_at = excluded.updated_at
             WHERE orchestrator_leases.holder_id = excluded.holder_id
                OR orchestrator_leases.lease_expires_at < excluded.updated_at",
            params![ORCHESTRATOR_LEASE_NAME, holder_id, expires_at, now],
        )?;
        Ok(affected > 0)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Release the global orchestrator lease if owned by the given holder.
pub fn release_orchestrator_lease(holder_id: &str) -> Result<(), TasksError> {
    with_db(|conn| {
        conn.execute(
            "DELETE FROM orchestrator_leases WHERE lease_name = ?1 AND holder_id = ?2",
            params![ORCHESTRATOR_LEASE_NAME, holder_id],
        )?;
        Ok(())
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}

/// Fetch current orchestrator lease, if present.
pub fn get_orchestrator_lease() -> Result<Option<OrchestratorLease>, TasksError> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT lease_name, holder_id, lease_expires_at, updated_at
             FROM orchestrator_leases
             WHERE lease_name = ?1",
        )?;

        let mut rows = stmt.query([ORCHESTRATOR_LEASE_NAME])?;
        if let Some(row) = rows.next()? {
            return Ok(Some(OrchestratorLease {
                lease_name: row.get(0)?,
                holder_id: row.get(1)?,
                lease_expires_at: row.get(2)?,
                updated_at: row.get(3)?,
            }));
        }
        Ok(None)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))
}
