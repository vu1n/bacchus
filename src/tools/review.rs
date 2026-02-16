//! Review tool - checks task completion before release
//!
//! Performs advisory checks on a task's work:
//! - Verifies workspace exists
//! - Runs build/test commands if specified
//! - Checks footprint compliance

use crate::db::with_db;
use crate::tasks;
use crate::workspace;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// Review check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCheck {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// Overall review output
#[derive(Debug, Serialize, Deserialize)]
pub struct ReviewOutput {
    pub task_id: String,
    pub passed: bool,
    pub checks: Vec<ReviewCheck>,
    pub summary: String,
}

/// Run a review on a task
pub fn review_task(
    task_id: &str,
    workspace_root: &Path,
    build_cmd: Option<&str>,
    test_cmd: Option<&str>,
) -> Result<ReviewOutput, String> {
    let mut checks = Vec::new();

    // 1. Check task exists and is in progress
    let task = match tasks::get_sqlite_task(task_id) {
        Ok(t) => t,
        Err(e) => {
            return Ok(ReviewOutput {
                task_id: task_id.to_string(),
                passed: false,
                checks: vec![ReviewCheck {
                    name: "Task exists".to_string(),
                    passed: false,
                    message: format!("Task not found: {}", e),
                }],
                summary: "Review failed: task not found".to_string(),
            });
        }
    };

    checks.push(ReviewCheck {
        name: "Task exists".to_string(),
        passed: true,
        message: format!(
            "Task {} found with status '{}'",
            task_id,
            task.status.as_str()
        ),
    });

    // 2. Check workspace exists
    let workspace_path = workspace::get_workspaces_dir(workspace_root).join(task_id);
    let workspace_exists = workspace_path.exists();

    checks.push(ReviewCheck {
        name: "Workspace exists".to_string(),
        passed: workspace_exists,
        message: if workspace_exists {
            format!("Workspace at {}", workspace_path.display())
        } else {
            "No workspace found".to_string()
        },
    });

    if !workspace_exists {
        return Ok(ReviewOutput {
            task_id: task_id.to_string(),
            passed: false,
            checks,
            summary: "Review failed: no workspace found".to_string(),
        });
    }

    // 3. Check for changes in workspace using jj
    let changes_check = check_workspace_changes(workspace_root, task_id);
    checks.push(changes_check.clone());

    // 4. Check footprint compliance
    let footprint_check = check_footprint_compliance(task_id, workspace_root, &workspace_path);
    checks.push(footprint_check);

    // 5. Run build command if specified
    if let Some(cmd) = build_cmd {
        let build_check = run_command_check("Build", cmd, &workspace_path);
        checks.push(build_check);
    }

    // 6. Run test command if specified
    if let Some(cmd) = test_cmd {
        let test_check = run_command_check("Test", cmd, &workspace_path);
        checks.push(test_check);
    }

    // Calculate overall pass/fail
    let passed = checks.iter().all(|c| c.passed);
    let failed_count = checks.iter().filter(|c| !c.passed).count();

    let summary = if passed {
        format!("All {} checks passed", checks.len())
    } else {
        format!("{} of {} checks failed", failed_count, checks.len())
    };

    Ok(ReviewOutput {
        task_id: task_id.to_string(),
        passed,
        checks,
        summary,
    })
}

/// Check if the workspace has changes
fn check_workspace_changes(workspace_root: &Path, task_id: &str) -> ReviewCheck {
    let workspace_path = workspace::get_workspaces_dir(workspace_root).join(task_id);

    // Use jj to check for changes
    let output = Command::new("jj")
        .args(["log", "-r", "@", "--no-graph", "-T", "change_id"])
        .current_dir(&workspace_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let change_id = String::from_utf8_lossy(&out.stdout).trim().to_string();

            // Check if there are any changes
            let diff_output = Command::new("jj")
                .args(["diff", "-r", "@", "--stat"])
                .current_dir(&workspace_path)
                .output();

            match diff_output {
                Ok(diff) if diff.status.success() => {
                    let diff_str = String::from_utf8_lossy(&diff.stdout);
                    let has_changes = !diff_str.trim().is_empty();

                    ReviewCheck {
                        name: "Changes exist".to_string(),
                        passed: has_changes,
                        message: if has_changes {
                            format!(
                                "Workspace has changes (change: {})",
                                &change_id[..8.min(change_id.len())]
                            )
                        } else {
                            "No changes in workspace".to_string()
                        },
                    }
                }
                _ => ReviewCheck {
                    name: "Changes exist".to_string(),
                    passed: false,
                    message: "Failed to check for changes".to_string(),
                },
            }
        }
        Ok(out) => ReviewCheck {
            name: "Changes exist".to_string(),
            passed: false,
            message: format!(
                "Failed to check workspace: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => ReviewCheck {
            name: "Changes exist".to_string(),
            passed: false,
            message: format!("Failed to run jj: {}", e),
        },
    }
}

/// Check if changes comply with declared footprint
fn check_footprint_compliance(
    task_id: &str,
    _workspace_root: &Path,
    workspace_path: &Path,
) -> ReviewCheck {
    // Get declared footprint
    let footprint: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare("SELECT file_path FROM task_footprints WHERE task_id = ?1")?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap_or_default();

    if footprint.is_empty() {
        return ReviewCheck {
            name: "Footprint compliance".to_string(),
            passed: true,
            message: "No footprint declared (all changes allowed)".to_string(),
        };
    }

    // Get files changed in workspace using jj
    let output = Command::new("jj")
        .args(["diff", "-r", "@", "--name-only"])
        .current_dir(workspace_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let changed_files: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|s| s.to_string())
                .collect();

            // Check if all changed files are in footprint
            let mut violations = Vec::new();
            for file in &changed_files {
                let normalized_file = file.trim_start_matches("./");
                let in_footprint = footprint
                    .iter()
                    .map(|fp| fp.trim_start_matches("./"))
                    .any(|fp| fp == normalized_file);
                if !in_footprint {
                    violations.push(file.clone());
                }
            }

            if violations.is_empty() {
                ReviewCheck {
                    name: "Footprint compliance".to_string(),
                    passed: true,
                    message: format!(
                        "All {} changed files within declared footprint",
                        changed_files.len()
                    ),
                }
            } else {
                ReviewCheck {
                    name: "Footprint compliance".to_string(),
                    passed: false,
                    message: format!(
                        "{} file(s) changed outside footprint: {}",
                        violations.len(),
                        violations.join(", ")
                    ),
                }
            }
        }
        Ok(out) => ReviewCheck {
            name: "Footprint compliance".to_string(),
            passed: false,
            message: format!(
                "Failed to get changed files: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => ReviewCheck {
            name: "Footprint compliance".to_string(),
            passed: false,
            message: format!("Failed to run jj: {}", e),
        },
    }
}

/// Run a command and return a check result
fn run_command_check(name: &str, cmd: &str, working_dir: &Path) -> ReviewCheck {
    let output = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", cmd])
            .current_dir(working_dir)
            .output()
    } else {
        Command::new("sh")
            .args(["-c", cmd])
            .current_dir(working_dir)
            .output()
    };

    match output {
        Ok(out) => {
            let passed = out.status.success();
            let message = if passed {
                format!("`{}` passed", cmd)
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let stdout = String::from_utf8_lossy(&out.stdout);
                format!(
                    "`{}` failed (exit code {:?})\n{}{}",
                    cmd,
                    out.status.code(),
                    if !stdout.is_empty() {
                        format!("stdout: {}\n", stdout.trim())
                    } else {
                        String::new()
                    },
                    if !stderr.is_empty() {
                        format!("stderr: {}", stderr.trim())
                    } else {
                        String::new()
                    }
                )
            };

            ReviewCheck {
                name: name.to_string(),
                passed,
                message,
            }
        }
        Err(e) => ReviewCheck {
            name: name.to_string(),
            passed: false,
            message: format!("Failed to run command: {}", e),
        },
    }
}
