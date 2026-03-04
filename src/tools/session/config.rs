//! Session configuration: default constants and environment-based overrides.

use crate::tasks;

pub(super) const DEFAULT_AGENT_HEARTBEAT_INTERVAL_MS: u64 = 30_000;
pub(super) const DEFAULT_ORCHESTRATOR_LEASE_RENEW_INTERVAL_MS: u64 = 30_000;
pub(super) const DEFAULT_WORKER_RETRY_BACKOFF_MS: i64 = 60_000;
pub(super) const DEFAULT_WORKER_MAX_RETRIES: i32 = 3;
pub(super) const DEFAULT_WORKER_STALE_GRACE_MS: i64 = 60_000;
pub(super) const DEFAULT_WORKER_KILL_STALE: bool = false;

/// Read a typed env var with validation, returning `default` when absent or invalid.
fn env_or<T: std::str::FromStr>(key: &str, default: T, valid: impl Fn(&T) -> bool) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<T>().ok())
        .filter(valid)
        .unwrap_or(default)
}

/// Read an optional typed env var with validation.
fn env_opt<T: std::str::FromStr>(key: &str, valid: impl Fn(&T) -> bool) -> Option<T> {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<T>().ok())
        .filter(valid)
}

/// Read a boolean env var (truthy unless "0"/"false"/"off"/"no").
fn env_bool(key: &str, default: bool) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        })
        .unwrap_or(default)
}

pub(super) fn configured_agent_heartbeat_interval_ms() -> u64 {
    env_or("BACCHUS_AGENT_HEARTBEAT_INTERVAL_MS", DEFAULT_AGENT_HEARTBEAT_INTERVAL_MS, |v| *v > 0)
}

pub(super) fn configured_orchestrator_lease_ttl_ms() -> i64 {
    env_or("BACCHUS_ORCHESTRATOR_LEASE_TTL_MS", tasks::ORCHESTRATOR_LEASE_TTL_MS, |v| *v > 0)
}

pub(super) fn configured_orchestrator_lease_interval_ms() -> u64 {
    env_or("BACCHUS_ORCHESTRATOR_LEASE_INTERVAL_MS", DEFAULT_ORCHESTRATOR_LEASE_RENEW_INTERVAL_MS, |v| *v > 0)
}

pub(super) fn configured_orchestrator_auto_spawn() -> bool {
    env_bool("BACCHUS_ORCHESTRATOR_AUTO_SPAWN", true)
}

pub(super) fn configured_worker_command() -> Option<String> {
    std::env::var("BACCHUS_WORKER_CMD")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(super) fn configured_worker_retry_backoff_ms() -> i64 {
    env_or("BACCHUS_WORKER_RETRY_BACKOFF_MS", DEFAULT_WORKER_RETRY_BACKOFF_MS, |v| *v > 0)
}

pub(super) fn configured_worker_max_retries() -> i32 {
    env_or("BACCHUS_WORKER_MAX_RETRIES", DEFAULT_WORKER_MAX_RETRIES, |v| *v > 0)
}

pub(super) fn configured_worker_stale_grace_ms() -> i64 {
    env_or("BACCHUS_WORKER_STALE_GRACE_MS", DEFAULT_WORKER_STALE_GRACE_MS, |v| *v >= 0)
}

pub(super) fn configured_worker_max_runtime_ms() -> Option<i64> {
    env_opt("BACCHUS_WORKER_MAX_RUNTIME_MS", |v| *v > 0)
}

pub(super) fn configured_worker_kill_stale() -> bool {
    env_bool("BACCHUS_WORKER_KILL_STALE", DEFAULT_WORKER_KILL_STALE)
}
