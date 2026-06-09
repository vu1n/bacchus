//! Quality gates and project-level quality configuration.
//!
//! Provides:
//! - `QualityConfig` loaded from `.bacchus/config.yaml`
//! - Pre-release quality gate (check, test, lint commands)

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use wait_timeout::ChildExt;

/// Project-level configuration from `.bacchus/config.yaml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacchusConfig {
    #[serde(default)]
    pub quality: QualitySection,
    #[serde(default)]
    pub worker: WorkerSection,
    #[serde(default)]
    pub memory: MemorySection,
}

/// kypp shared-memory integration (briefing/recall/remember for workers).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemorySection {
    /// Enable kypp integration: scope worker env to a kypp project so
    /// `kypp briefing/recall/remember` bind the right store.
    #[serde(default)]
    pub enabled: bool,
    /// kypp project key (KYPP_PROJECT). Defaults to the project directory name.
    pub project: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSection {
    /// Worker command (e.g., "claude"). Overrides the runner default when set.
    pub cmd: Option<String>,
    /// Runner family: "claude" (default) or "codex". Selects the default worker
    /// command when `cmd` is unset. See `default_cmd_for_runner`.
    pub runner: Option<String>,
    /// Whether auto-spawn is enabled (overrides env var)
    #[serde(default = "default_true")]
    pub auto_spawn: bool,
    /// Retry backoff in milliseconds
    pub retry_backoff_ms: Option<i64>,
    /// Maximum number of retries before blocking the task
    pub max_retries: Option<i32>,
    /// Grace period before considering a worker stale (ms)
    pub stale_grace_ms: Option<i64>,
    /// Maximum runtime before a worker is considered stale (ms)
    pub max_runtime_ms: Option<i64>,
    /// Whether to kill stale workers
    #[serde(default)]
    pub kill_stale: bool,
}

fn default_true() -> bool {
    true
}

impl Default for WorkerSection {
    fn default() -> Self {
        Self {
            cmd: None,
            runner: None,
            auto_spawn: true,
            retry_backoff_ms: None,
            max_retries: None,
            stale_grace_ms: None,
            max_runtime_ms: None,
            kill_stale: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QualitySection {
    /// Build/compile check command (e.g., "cargo check --quiet")
    pub check: Option<String>,
    /// Test command (e.g., "cargo test --quiet")
    pub test: Option<String>,
    /// Lint command (e.g., "cargo clippy --quiet -- -D warnings")
    pub lint: Option<String>,
}

/// Result of running the quality gate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityGateResult {
    pub passed: bool,
    pub checks: Vec<QualityCheck>,
}

/// Result of a single quality check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCheck {
    pub name: String,
    pub passed: bool,
    pub output: String,
}

const CONFIG_FILENAME: &str = "config.yaml";
const GATE_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

/// Load project config from `.bacchus/config.yaml`. Returns None if missing or unparseable.
pub fn load_config(workspace_root: &Path) -> Option<BacchusConfig> {
    let path = workspace_root.join(".bacchus").join(CONFIG_FILENAME);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_yaml::from_str(&content).ok()
}

/// Resolve kypp env for a worker: `(KYPP_PROJECT, KYPP_REPO_ROOT)`.
///
/// Returns `None` when memory is disabled. The project key is the explicit
/// `memory.project`, else the workspace directory name (used verbatim as the
/// kypp binding key). Code grounding (`KYPP_REPO_ROOT`) points at the canonical
/// project tree, not the ephemeral per-task workspace.
pub fn resolve_memory_env(
    config: &BacchusConfig,
    workspace_root: &Path,
) -> Option<(String, String)> {
    if !config.memory.enabled {
        return None;
    }
    let project = config.memory.project.clone().unwrap_or_else(|| {
        workspace_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "default".to_string())
    });
    Some((project, workspace_root.to_string_lossy().to_string()))
}

/// Run all configured quality gate commands against the workspace path.
///
/// Commands run sequentially with cwd set to `workspace_path`.
/// Stops on first failure (short-circuit).
pub fn run_quality_gate(
    config: &BacchusConfig,
    workspace_path: &Path,
) -> Result<QualityGateResult, String> {
    let mut checks = Vec::new();
    let mut all_passed = true;

    let gates = [
        ("check", &config.quality.check),
        ("test", &config.quality.test),
        ("lint", &config.quality.lint),
    ];

    for (name, cmd_opt) in &gates {
        if let Some(cmd) = cmd_opt {
            let check = run_command(name, cmd, workspace_path)?;
            if !check.passed {
                all_passed = false;
                checks.push(check);
                break; // short-circuit on first failure
            }
            checks.push(check);
        }
    }

    Ok(QualityGateResult {
        passed: all_passed,
        checks,
    })
}

/// Run a single shell command with timeout and capture output.
fn run_command(name: &str, cmd: &str, cwd: &Path) -> Result<QualityCheck, String> {
    let child = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", cmd])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    } else {
        Command::new("sh")
            .args(["-c", cmd])
            .current_dir(cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
    };

    let mut child = child.map_err(|e| format!("Failed to spawn '{}': {}", cmd, e))?;

    let result = child
        .wait_timeout(GATE_TIMEOUT)
        .map_err(|e| format!("Failed waiting for '{}': {}", name, e))?;

    match result {
        Some(status) => {
            let stdout = child
                .stdout
                .take()
                .map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf
                })
                .unwrap_or_default();
            let stderr = child
                .stderr
                .take()
                .map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf
                })
                .unwrap_or_default();

            let output = if stderr.is_empty() {
                stdout
            } else {
                format!("{}\n{}", stdout, stderr).trim().to_string()
            };

            Ok(QualityCheck {
                name: name.to_string(),
                passed: status.success(),
                output: truncate_output(&output, 2000),
            })
        }
        None => {
            // Timeout — kill the process
            let _ = child.kill();
            Ok(QualityCheck {
                name: name.to_string(),
                passed: false,
                output: format!("Timed out after {}s", GATE_TIMEOUT.as_secs()),
            })
        }
    }
}

/// Truncate output to max chars, appending "... (truncated)" if needed.
fn truncate_output(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}... (truncated)", &s[..max])
    }
}

/// Persist quality gate check results to the DB for audit/debugging.
///
/// Truncates output to 4KB per check to keep the DB lean.
pub fn store_quality_results(task_id: &str, checks: &[QualityCheck]) {
    let now = chrono::Utc::now().timestamp_millis();
    let _ = crate::db::with_db(|conn| {
        for check in checks {
            let output = truncate_output(&check.output, 4096);
            conn.execute(
                "INSERT OR REPLACE INTO task_quality_results (task_id, check_name, passed, output, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![task_id, check.name, check.passed as i32, output, now],
            )?;
        }
        Ok(())
    });
}

/// Load quality gate results from DB for a given task.
pub fn load_quality_results(task_id: &str) -> Vec<QualityCheck> {
    crate::db::with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT check_name, passed, output FROM task_quality_results WHERE task_id = ?1 ORDER BY check_name",
        )?;
        let rows = stmt
            .query_map([task_id], |row| {
                Ok(QualityCheck {
                    name: row.get(0)?,
                    passed: row.get::<_, i32>(1)? != 0,
                    output: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap_or_default()
}

/// Format quality gate failures for user display.
pub fn format_gate_failures(gate: &QualityGateResult) -> String {
    let mut msg = String::from("Quality gate failed:\n");
    for check in &gate.checks {
        let icon = if check.passed { "PASS" } else { "FAIL" };
        msg.push_str(&format!("  [{}] {}", icon, check.name));
        if !check.passed && !check.output.is_empty() {
            msg.push_str(&format!("\n    {}", check.output.replace('\n', "\n    ")));
        }
        msg.push('\n');
    }
    msg
}

// ============================================================================
// Config Generation (for bacchus init)
// ============================================================================

/// Default worker command for a runner family. A `cmd` set in config overrides this.
///
/// - `codex` runs headless via `codex exec`, fed the protocol prompt that
///   `bacchus worker-prompt` emits (codex has no `/bacchus-worker` slash command).
/// - anything else (incl. `None`) defaults to the Claude Code slash-command worker.
pub fn default_cmd_for_runner(runner: Option<&str>) -> String {
    match runner {
        // `run_worker_command` substitutes $BACCHUS_AGENT_ID/$BACCHUS_TASK_ID before
        // `sh -c`, so the command substitution runs with concrete IDs. The double
        // quotes keep the prompt (which itself contains $BACCHUS_* and backticks) as
        // one un-re-expanded argument to `codex exec`.
        Some("codex") => "codex exec --dangerously-bypass-approvals-and-sandbox \
             \"$(bacchus worker-prompt $BACCHUS_AGENT_ID $BACCHUS_TASK_ID)\""
            .to_string(),
        _ => "claude --dangerously-skip-permissions -p \
             '/bacchus-worker $BACCHUS_AGENT_ID $BACCHUS_TASK_ID'"
            .to_string(),
    }
}

/// Detect project type and generate default config YAML content (quality + worker sections).
pub fn generate_config(workspace_root: &Path, runner: &str) -> String {
    let (check, test, lint) = detect_project_commands(workspace_root);
    let mut yaml = String::from("quality:\n");
    if let Some(c) = check {
        yaml.push_str(&format!("  check: \"{}\"\n", c));
    }
    if let Some(t) = test {
        yaml.push_str(&format!("  test: \"{}\"\n", t));
    }
    if let Some(l) = lint {
        yaml.push_str(&format!("  lint: \"{}\"\n", l));
    }
    yaml.push_str("\nworker:\n");
    yaml.push_str(&format!("  runner: \"{}\"\n", runner));
    // YAML double-quoted scalar: escape embedded quotes (the codex cmd uses them).
    let cmd = default_cmd_for_runner(Some(runner)).replace('"', "\\\"");
    yaml.push_str(&format!("  cmd: \"{}\"\n", cmd));
    yaml.push_str(
        "\n# Shared memory via kypp (briefing/recall/remember). Requires `kypp` on PATH.\n",
    );
    yaml.push_str("# memory:\n");
    yaml.push_str("#   enabled: true\n");
    yaml.push_str(
        "#   project: \"my-project\"   # KYPP_PROJECT; defaults to this directory's name\n",
    );
    yaml
}

/// Detect project type from marker files and return (check, test, lint) commands.
fn detect_project_commands(
    workspace_root: &Path,
) -> (Option<String>, Option<String>, Option<String>) {
    // Rust
    if workspace_root.join("Cargo.toml").exists() {
        return (
            Some("cargo check --quiet".to_string()),
            Some("cargo test --quiet".to_string()),
            Some("cargo clippy --quiet -- -D warnings".to_string()),
        );
    }

    // Node.js / TypeScript
    if workspace_root.join("package.json").exists() {
        // Check for biome vs eslint
        let lint = if workspace_root.join("biome.json").exists()
            || workspace_root.join("biome.jsonc").exists()
        {
            Some("npx biome check .".to_string())
        } else if workspace_root.join(".eslintrc.json").exists()
            || workspace_root.join(".eslintrc.js").exists()
            || workspace_root.join("eslint.config.js").exists()
        {
            Some("npx eslint .".to_string())
        } else {
            None
        };

        // Check for bun vs npm
        let runner = if workspace_root.join("bun.lockb").exists()
            || workspace_root.join("bun.lock").exists()
        {
            "bun"
        } else {
            "npx"
        };

        return (
            Some(format!("{} tsc --noEmit", runner)),
            Some(format!("{} vitest run", runner)),
            lint,
        );
    }

    // Go
    if workspace_root.join("go.mod").exists() {
        return (
            Some("go build ./...".to_string()),
            Some("go test ./...".to_string()),
            Some("go vet ./...".to_string()),
        );
    }

    // Python
    if workspace_root.join("pyproject.toml").exists() || workspace_root.join("setup.py").exists() {
        return (
            None,
            Some("pytest".to_string()),
            Some("ruff check .".to_string()),
        );
    }

    // Fallback — no detection
    (None, None, None)
}

// ============================================================================
// Duplicate Symbol Detection (for orchestrator)
// ============================================================================

/// A pair of duplicate symbols found across files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateSymbol {
    pub new_file: String,
    pub new_symbol: String,
    pub existing_file: String,
    pub existing_symbol: String,
    pub hash: String,
}

/// Detect duplicate symbols by hash between the given files and the rest of the index.
///
/// For each symbol in `changed_files`, checks if a symbol with the same hash exists
/// in a different file in the symbols table.
pub fn detect_duplicate_symbols(changed_files: &[String]) -> Vec<DuplicateSymbol> {
    if changed_files.is_empty() {
        return Vec::new();
    }

    crate::db::with_db(|conn| {
        let mut duplicates = Vec::new();

        for file in changed_files {
            let mut stmt = conn.prepare(
                "SELECT s1.fq_name, s1.hash, s2.file, s2.fq_name
                 FROM symbols s1
                 JOIN symbols s2 ON s1.hash = s2.hash AND s1.file != s2.file
                 WHERE s1.file = ?1
                   AND s1.hash IS NOT NULL
                   AND s1.hash != ''
                   AND s2.file NOT IN (SELECT value FROM json_each(?2))",
            )?;

            let changed_json =
                serde_json::to_string(changed_files).unwrap_or_else(|_| "[]".to_string());

            let rows = stmt.query_map(rusqlite::params![file, changed_json], |row| {
                Ok(DuplicateSymbol {
                    new_file: file.clone(),
                    new_symbol: row.get(0)?,
                    hash: row.get(1)?,
                    existing_file: row.get(2)?,
                    existing_symbol: row.get(3)?,
                })
            })?;

            for dup in rows.flatten() {
                duplicates.push(dup);
            }
        }

        Ok(duplicates)
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_config_missing() {
        let dir = tempdir().unwrap();
        assert!(load_config(dir.path()).is_none());
    }

    #[test]
    fn test_load_config_valid() {
        let dir = tempdir().unwrap();
        let bacchus_dir = dir.path().join(".bacchus");
        std::fs::create_dir_all(&bacchus_dir).unwrap();
        std::fs::write(
            bacchus_dir.join("config.yaml"),
            "quality:\n  check: \"cargo check\"\n  test: \"cargo test\"\n",
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.quality.check.as_deref(), Some("cargo check"));
        assert_eq!(config.quality.test.as_deref(), Some("cargo test"));
        assert!(config.quality.lint.is_none());
        // Worker section should default
        assert!(config.worker.cmd.is_none());
        assert!(config.worker.auto_spawn);
    }

    #[test]
    fn test_load_config_with_worker_section() {
        let dir = tempdir().unwrap();
        let bacchus_dir = dir.path().join(".bacchus");
        std::fs::create_dir_all(&bacchus_dir).unwrap();
        std::fs::write(
            bacchus_dir.join("config.yaml"),
            "quality:\n  check: \"cargo check\"\nworker:\n  cmd: \"claude\"\n  kill_stale: true\n",
        )
        .unwrap();

        let config = load_config(dir.path()).unwrap();
        assert_eq!(config.worker.cmd.as_deref(), Some("claude"));
        assert!(config.worker.kill_stale);
        assert!(config.worker.auto_spawn); // default
    }

    #[test]
    fn test_generate_config_rust() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )
        .unwrap();
        let yaml = generate_config(dir.path(), "claude");
        assert!(yaml.contains("cargo check"));
        assert!(yaml.contains("cargo test"));
        assert!(yaml.contains("cargo clippy"));
        assert!(yaml.contains("worker:"));
        assert!(yaml.contains("runner: \"claude\""));
        assert!(yaml.contains("cmd: \"claude --dangerously-skip-permissions -p"));
    }

    #[test]
    fn test_generate_config_codex_runner() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )
        .unwrap();
        let yaml = generate_config(dir.path(), "codex");
        assert!(yaml.contains("runner: \"codex\""));
        // The emitted scalar must escape the inner double quotes (the codex cmd
        // wraps the prompt in `"$(...)"`), or YAML parsing truncates the value.
        assert!(
            yaml.contains(r#"cmd: "codex exec --dangerously-bypass-approvals-and-sandbox \"$("#)
        );

        // Round-trip: the YAML-escaped cmd must parse back to the EXACT command,
        // inner `"$(bacchus worker-prompt ...)"` quotes intact. This pins the
        // `.replace('"', "\\\"")` escaping in generate_config.
        let bacchus_dir = dir.path().join(".bacchus");
        std::fs::create_dir_all(&bacchus_dir).unwrap();
        std::fs::write(bacchus_dir.join("config.yaml"), &yaml).unwrap();
        let cfg = load_config(dir.path()).unwrap();
        assert_eq!(cfg.worker.runner.as_deref(), Some("codex"));
        assert_eq!(
            cfg.worker.cmd.as_deref(),
            Some(default_cmd_for_runner(Some("codex")).as_str())
        );
        let cmd = cfg.worker.cmd.unwrap();
        assert!(cmd.contains(r#""$(bacchus worker-prompt $BACCHUS_AGENT_ID $BACCHUS_TASK_ID)""#));
    }

    #[test]
    fn test_generate_config_node() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let yaml = generate_config(dir.path(), "claude");
        assert!(yaml.contains("tsc --noEmit"));
        assert!(yaml.contains("vitest"));
        assert!(yaml.contains("cmd: \"claude --dangerously-skip-permissions -p"));
    }

    #[test]
    fn test_generate_config_go() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module test\n").unwrap();
        let yaml = generate_config(dir.path(), "claude");
        assert!(yaml.contains("go build"));
        assert!(yaml.contains("go test"));
        assert!(yaml.contains("cmd: \"claude --dangerously-skip-permissions -p"));
    }

    #[test]
    fn test_format_gate_failures() {
        let gate = QualityGateResult {
            passed: false,
            checks: vec![
                QualityCheck {
                    name: "check".to_string(),
                    passed: true,
                    output: String::new(),
                },
                QualityCheck {
                    name: "test".to_string(),
                    passed: false,
                    output: "test failed: assertion error".to_string(),
                },
            ],
        };
        let msg = format_gate_failures(&gate);
        assert!(msg.contains("[PASS] check"));
        assert!(msg.contains("[FAIL] test"));
        assert!(msg.contains("assertion error"));
    }

    #[test]
    fn test_run_quality_gate_passes() {
        let dir = tempdir().unwrap();
        let config = BacchusConfig {
            quality: QualitySection {
                check: Some("true".to_string()),
                test: Some("true".to_string()),
                lint: None,
            },
            worker: WorkerSection::default(),
            memory: MemorySection::default(),
        };
        let result = run_quality_gate(&config, dir.path()).unwrap();
        assert!(result.passed);
        assert_eq!(result.checks.len(), 2);
    }

    #[test]
    fn test_run_quality_gate_fails() {
        let dir = tempdir().unwrap();
        let config = BacchusConfig {
            quality: QualitySection {
                check: Some("true".to_string()),
                test: Some("false".to_string()),
                lint: Some("true".to_string()),
            },
            worker: WorkerSection::default(),
            memory: MemorySection::default(),
        };
        let result = run_quality_gate(&config, dir.path()).unwrap();
        assert!(!result.passed);
        // Should short-circuit: check passes, test fails, lint never runs
        assert_eq!(result.checks.len(), 2);
        assert!(result.checks[0].passed);
        assert!(!result.checks[1].passed);
    }

    #[test]
    fn test_resolve_memory_env_disabled_by_default() {
        let config = BacchusConfig {
            quality: QualitySection::default(),
            worker: WorkerSection::default(),
            memory: MemorySection::default(),
        };
        assert!(resolve_memory_env(&config, Path::new("/tmp/my-proj")).is_none());
    }

    #[test]
    fn test_resolve_memory_env_derives_project_from_dir() {
        let config = BacchusConfig {
            quality: QualitySection::default(),
            worker: WorkerSection::default(),
            memory: MemorySection {
                enabled: true,
                project: None,
            },
        };
        let (project, repo_root) = resolve_memory_env(&config, Path::new("/tmp/my-proj")).unwrap();
        assert_eq!(project, "my-proj");
        assert_eq!(repo_root, "/tmp/my-proj");
    }

    #[test]
    fn test_resolve_memory_env_explicit_project_wins() {
        let config = BacchusConfig {
            quality: QualitySection::default(),
            worker: WorkerSection::default(),
            memory: MemorySection {
                enabled: true,
                project: Some("custom-key".to_string()),
            },
        };
        let (project, _) = resolve_memory_env(&config, Path::new("/tmp/my-proj")).unwrap();
        assert_eq!(project, "custom-key");
    }

    #[test]
    fn test_truncate_output() {
        assert_eq!(truncate_output("short", 100), "short");
        assert_eq!(truncate_output("hello world", 5), "hello... (truncated)");
    }
}
