//! Quality gates and project-level quality configuration.
//!
//! Provides:
//! - `QualityConfig` loaded from `.bacchus/config.yaml`
//! - Pre-release quality gate (check, test, lint commands)
//! - Post-session desloppify scan integration

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use wait_timeout::ChildExt;

/// Quality configuration from `.bacchus/config.yaml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityConfig {
    #[serde(default)]
    pub quality: QualitySection,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QualitySection {
    /// Build/compile check command (e.g., "cargo check --quiet")
    pub check: Option<String>,
    /// Test command (e.g., "cargo test --quiet")
    pub test: Option<String>,
    /// Lint command (e.g., "cargo clippy --quiet -- -D warnings")
    pub lint: Option<String>,
    /// Whether to run desloppify scan on session stop
    #[serde(default)]
    pub desloppify: bool,
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

/// Load quality config from `.bacchus/config.yaml`. Returns None if missing or unparseable.
pub fn load_quality_config(workspace_root: &Path) -> Option<QualityConfig> {
    let path = workspace_root.join(".bacchus").join(CONFIG_FILENAME);
    let content = std::fs::read_to_string(&path).ok()?;
    serde_yaml::from_str(&content).ok()
}

/// Run all configured quality gate commands against the workspace path.
///
/// Commands run sequentially with cwd set to `workspace_path`.
/// Stops on first failure (short-circuit).
pub fn run_quality_gate(
    config: &QualityConfig,
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
// Desloppify Integration
// ============================================================================

/// Result of a desloppify scan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesloppifyScanResult {
    pub ran: bool,
    pub findings_count: usize,
    pub report_path: Option<String>,
    pub error: Option<String>,
}

const DESLOPPIFY_TIMEOUT: Duration = Duration::from_secs(600); // 10 minutes

/// Run desloppify mechanical scan if configured and available.
///
/// Non-blocking: returns results but never causes the caller to fail.
pub fn run_desloppify_scan(workspace_root: &Path) -> DesloppifyScanResult {
    let config = match load_quality_config(workspace_root) {
        Some(c) if c.quality.desloppify => c,
        _ => {
            return DesloppifyScanResult {
                ran: false,
                findings_count: 0,
                report_path: None,
                error: None,
            }
        }
    };
    let _ = config; // used only for the desloppify check above

    // Check if desloppify binary is available
    let available = Command::new("which")
        .arg("desloppify")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !available {
        return DesloppifyScanResult {
            ran: false,
            findings_count: 0,
            report_path: None,
            error: Some("desloppify binary not found".to_string()),
        };
    }

    // Run desloppify scan
    let child = Command::new("desloppify")
        .args(["scan", "--path", "."])
        .current_dir(workspace_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            return DesloppifyScanResult {
                ran: false,
                findings_count: 0,
                report_path: None,
                error: Some(format!("Failed to spawn desloppify: {}", e)),
            }
        }
    };

    let result = child.wait_timeout(DESLOPPIFY_TIMEOUT).ok().flatten();

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

            // Try to parse findings from stdout/state file
            let findings_count = parse_desloppify_findings(&stdout);

            // Write quality report
            let report_path = workspace_root.join(".bacchus/quality-report.json");
            let report = serde_json::json!({
                "scan_type": "desloppify",
                "success": status.success(),
                "findings_count": findings_count,
                "raw_output": truncate_output(&stdout, 5000),
                "timestamp": chrono::Utc::now().to_rfc3339(),
            });
            let _ = std::fs::write(&report_path, serde_json::to_string_pretty(&report).unwrap_or_default());

            DesloppifyScanResult {
                ran: true,
                findings_count,
                report_path: Some(report_path.to_string_lossy().to_string()),
                error: if status.success() {
                    None
                } else {
                    Some(format!("desloppify exited with code {:?}", status.code()))
                },
            }
        }
        None => {
            let _ = child.kill();
            DesloppifyScanResult {
                ran: true,
                findings_count: 0,
                report_path: None,
                error: Some(format!(
                    "desloppify timed out after {}s",
                    DESLOPPIFY_TIMEOUT.as_secs()
                )),
            }
        }
    }
}

/// Parse finding count from desloppify output.
/// Looks for JSON with "findings" array or "count" field, falls back to line counting.
fn parse_desloppify_findings(output: &str) -> usize {
    // Try JSON parse first
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(output) {
        if let Some(arr) = val.get("findings").and_then(|f| f.as_array()) {
            return arr.len();
        }
        if let Some(count) = val.get("count").and_then(|c| c.as_u64()) {
            return count as usize;
        }
    }
    // Fallback: count non-empty lines that look like findings
    output
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .count()
}

// ============================================================================
// Config Generation (for bacchus init)
// ============================================================================

/// Detect project type and generate default quality config YAML content.
pub fn generate_quality_config(workspace_root: &Path) -> String {
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
    yaml.push_str("  desloppify: true\n");
    yaml
}

/// Detect project type from marker files and return (check, test, lint) commands.
fn detect_project_commands(workspace_root: &Path) -> (Option<String>, Option<String>, Option<String>) {
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
    if workspace_root.join("pyproject.toml").exists()
        || workspace_root.join("setup.py").exists()
    {
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

            let changed_json = serde_json::to_string(changed_files).unwrap_or_else(|_| "[]".to_string());

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
    fn test_load_quality_config_missing() {
        let dir = tempdir().unwrap();
        assert!(load_quality_config(dir.path()).is_none());
    }

    #[test]
    fn test_load_quality_config_valid() {
        let dir = tempdir().unwrap();
        let bacchus_dir = dir.path().join(".bacchus");
        std::fs::create_dir_all(&bacchus_dir).unwrap();
        std::fs::write(
            bacchus_dir.join("config.yaml"),
            "quality:\n  check: \"cargo check\"\n  test: \"cargo test\"\n  desloppify: true\n",
        )
        .unwrap();

        let config = load_quality_config(dir.path()).unwrap();
        assert_eq!(config.quality.check.as_deref(), Some("cargo check"));
        assert_eq!(config.quality.test.as_deref(), Some("cargo test"));
        assert!(config.quality.lint.is_none());
        assert!(config.quality.desloppify);
    }

    #[test]
    fn test_generate_quality_config_rust() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();
        let yaml = generate_quality_config(dir.path());
        assert!(yaml.contains("cargo check"));
        assert!(yaml.contains("cargo test"));
        assert!(yaml.contains("cargo clippy"));
    }

    #[test]
    fn test_generate_quality_config_node() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let yaml = generate_quality_config(dir.path());
        assert!(yaml.contains("tsc --noEmit"));
        assert!(yaml.contains("vitest"));
    }

    #[test]
    fn test_generate_quality_config_go() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module test\n").unwrap();
        let yaml = generate_quality_config(dir.path());
        assert!(yaml.contains("go build"));
        assert!(yaml.contains("go test"));
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
        let config = QualityConfig {
            quality: QualitySection {
                check: Some("true".to_string()),
                test: Some("true".to_string()),
                lint: None,
                desloppify: false,
            },
        };
        let result = run_quality_gate(&config, dir.path()).unwrap();
        assert!(result.passed);
        assert_eq!(result.checks.len(), 2);
    }

    #[test]
    fn test_run_quality_gate_fails() {
        let dir = tempdir().unwrap();
        let config = QualityConfig {
            quality: QualitySection {
                check: Some("true".to_string()),
                test: Some("false".to_string()),
                lint: Some("true".to_string()),
                desloppify: false,
            },
        };
        let result = run_quality_gate(&config, dir.path()).unwrap();
        assert!(!result.passed);
        // Should short-circuit: check passes, test fails, lint never runs
        assert_eq!(result.checks.len(), 2);
        assert!(result.checks[0].passed);
        assert!(!result.checks[1].passed);
    }

    #[test]
    fn test_truncate_output() {
        assert_eq!(truncate_output("short", 100), "short");
        assert_eq!(truncate_output("hello world", 5), "hello... (truncated)");
    }

    #[test]
    fn test_parse_desloppify_findings_json() {
        let json = r#"{"findings": [{"type": "dupe"}, {"type": "unused"}]}"#;
        assert_eq!(parse_desloppify_findings(json), 2);
    }

    #[test]
    fn test_parse_desloppify_findings_count() {
        let json = r#"{"count": 5}"#;
        assert_eq!(parse_desloppify_findings(json), 5);
    }
}
