//! Session configuration: default constants and environment-based overrides.

use crate::tasks;

pub(super) const DEFAULT_AGENT_HEARTBEAT_INTERVAL_MS: u64 = 30_000;
pub(super) const DEFAULT_ORCHESTRATOR_LEASE_RENEW_INTERVAL_MS: u64 = 30_000;
pub(super) const DEFAULT_WORKER_RETRY_BACKOFF_MS: i64 = 60_000;
pub(super) const DEFAULT_WORKER_MAX_RETRIES: i32 = 3;
pub(super) const DEFAULT_WORKER_STALE_GRACE_MS: i64 = 60_000;
pub(super) const DEFAULT_WORKER_KILL_STALE: bool = false;
pub(super) const DEFAULT_EVENT_POLL_TIMEOUT_MS: u32 = 28_000;

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
    env_or(
        "BACCHUS_AGENT_HEARTBEAT_INTERVAL_MS",
        DEFAULT_AGENT_HEARTBEAT_INTERVAL_MS,
        |v| *v > 0,
    )
}

pub(super) fn configured_orchestrator_lease_ttl_ms() -> i64 {
    env_or(
        "BACCHUS_ORCHESTRATOR_LEASE_TTL_MS",
        tasks::ORCHESTRATOR_LEASE_TTL_MS,
        |v| *v > 0,
    )
}

pub(super) fn configured_orchestrator_lease_interval_ms() -> u64 {
    env_or(
        "BACCHUS_ORCHESTRATOR_LEASE_INTERVAL_MS",
        DEFAULT_ORCHESTRATOR_LEASE_RENEW_INTERVAL_MS,
        |v| *v > 0,
    )
}

pub(super) fn configured_event_poll_timeout_ms() -> u32 {
    env_or(
        "BACCHUS_EVENT_POLL_TIMEOUT_MS",
        DEFAULT_EVENT_POLL_TIMEOUT_MS,
        |v| *v > 0,
    )
}

pub(super) fn configured_orchestrator_auto_spawn(worker_cfg: &ResolvedWorkerConfig) -> bool {
    // Env var overrides config.yaml
    if std::env::var("BACCHUS_ORCHESTRATOR_AUTO_SPAWN").is_ok() {
        return env_bool("BACCHUS_ORCHESTRATOR_AUTO_SPAWN", true);
    }
    worker_cfg.auto_spawn
}

/// All worker settings resolved once from env vars + config.yaml.
///
/// Env vars override config.yaml values. Config.yaml overrides built-in defaults.
/// Load once per entry point via `resolve_worker_config()`, then pass by reference.
#[derive(Debug)]
pub(super) struct ResolvedWorkerConfig {
    pub cmd: Option<String>,
    pub auto_spawn: bool,
    pub retry_backoff_ms: i64,
    pub max_retries: i32,
    pub stale_grace_ms: i64,
    pub max_runtime_ms: Option<i64>,
    pub kill_stale: bool,
}

/// Load worker config once: env vars → config.yaml → built-in defaults.
pub(super) fn resolve_worker_config(
    workspace_root: Option<&std::path::Path>,
) -> ResolvedWorkerConfig {
    let yaml_worker = workspace_root
        .and_then(crate::quality::load_config)
        .map(|c| c.worker);

    ResolvedWorkerConfig {
        cmd: std::env::var("BACCHUS_WORKER_CMD")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .or_else(|| yaml_worker.as_ref().and_then(|w| w.cmd.clone())),

        auto_spawn: if std::env::var("BACCHUS_ORCHESTRATOR_AUTO_SPAWN").is_ok() {
            env_bool("BACCHUS_ORCHESTRATOR_AUTO_SPAWN", true)
        } else {
            yaml_worker.as_ref().map(|w| w.auto_spawn).unwrap_or(true)
        },

        retry_backoff_ms: env_opt("BACCHUS_WORKER_RETRY_BACKOFF_MS", |v: &i64| *v > 0)
            .or_else(|| yaml_worker.as_ref()?.retry_backoff_ms.filter(|v| *v > 0))
            .unwrap_or(DEFAULT_WORKER_RETRY_BACKOFF_MS),

        max_retries: env_opt("BACCHUS_WORKER_MAX_RETRIES", |v: &i32| *v > 0)
            .or_else(|| yaml_worker.as_ref()?.max_retries.filter(|v| *v > 0))
            .unwrap_or(DEFAULT_WORKER_MAX_RETRIES),

        stale_grace_ms: env_opt("BACCHUS_WORKER_STALE_GRACE_MS", |v: &i64| *v >= 0)
            .or_else(|| yaml_worker.as_ref()?.stale_grace_ms.filter(|v| *v >= 0))
            .unwrap_or(DEFAULT_WORKER_STALE_GRACE_MS),

        max_runtime_ms: env_opt("BACCHUS_WORKER_MAX_RUNTIME_MS", |v: &i64| *v > 0)
            .or_else(|| yaml_worker.as_ref()?.max_runtime_ms.filter(|v| *v > 0)),

        kill_stale: if let Some(v) = env_opt("BACCHUS_WORKER_KILL_STALE", |_: &bool| true) {
            v
        } else {
            yaml_worker
                .as_ref()
                .map(|w| w.kill_stale)
                .unwrap_or(DEFAULT_WORKER_KILL_STALE)
        },
    }
}
