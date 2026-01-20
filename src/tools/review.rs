//! Review tool - checks task completion before release
//!
//! Performs advisory checks on a task's work:
//! - Verifies commits exist in the worktree branch
//! - Runs build/test commands if specified
//! - Checks footprint compliance

use crate::db::with_db;
use crate::tasks;
use crate::worktree;
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
        message: format!("Task {} found with status '{}'", task_id, task.status.as_str()),
    });

    // 2. Check worktree exists
    let worktree_path = worktree::get_worktrees_dir(workspace_root).join(task_id);
    let worktree_exists = worktree_path.exists();

    checks.push(ReviewCheck {
        name: "Worktree exists".to_string(),
        passed: worktree_exists,
        message: if worktree_exists {
            format!("Worktree at {}", worktree_path.display())
        } else {
            "No worktree found".to_string()
        },
    });

    if !worktree_exists {
        return Ok(ReviewOutput {
            task_id: task_id.to_string(),
            passed: false,
            checks,
            summary: "Review failed: no worktree found".to_string(),
        });
    }

    // 3. Check for commits
    let branch_name = format!("bacchus/{}", task_id);
    let commits_check = check_branch_commits(workspace_root, &branch_name);
    checks.push(commits_check.clone());

    // 4. Check footprint compliance
    let footprint_check = check_footprint_compliance(task_id, workspace_root, &branch_name);
    checks.push(footprint_check);

    // 5. Run build command if specified
    if let Some(cmd) = build_cmd {
        let build_check = run_command_check("Build", cmd, &worktree_path);
        checks.push(build_check);
    }

    // 6. Run test command if specified
    if let Some(cmd) = test_cmd {
        let test_check = run_command_check("Test", cmd, &worktree_path);
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

/// Check if the branch has commits beyond main
fn check_branch_commits(workspace_root: &Path, branch_name: &str) -> ReviewCheck {
    // Count commits on branch that aren't on main
    let output = Command::new("git")
        .args(["rev-list", "--count", &format!("main..{}", branch_name)])
        .current_dir(workspace_root)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let count_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let count: i32 = count_str.parse().unwrap_or(0);

            ReviewCheck {
                name: "Commits exist".to_string(),
                passed: count > 0,
                message: if count > 0 {
                    format!("{} commit(s) on branch {}", count, branch_name)
                } else {
                    format!("No commits on branch {} beyond main", branch_name)
                },
            }
        }
        Ok(out) => ReviewCheck {
            name: "Commits exist".to_string(),
            passed: false,
            message: format!(
                "Failed to check commits: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => ReviewCheck {
            name: "Commits exist".to_string(),
            passed: false,
            message: format!("Failed to run git: {}", e),
        },
    }
}

/// Check if changes comply with declared footprint
fn check_footprint_compliance(
    task_id: &str,
    workspace_root: &Path,
    branch_name: &str,
) -> ReviewCheck {
    // Get declared footprint
    let footprint: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT file_path FROM task_footprints WHERE task_id = ?1",
        )?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
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

    // Get files changed on the branch
    let output = Command::new("git")
        .args(["diff", "--name-only", &format!("main...{}", branch_name)])
        .current_dir(workspace_root)
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
                let in_footprint = footprint.iter().any(|fp| file.starts_with(fp) || fp == file);
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
            message: format!("Failed to run git: {}", e),
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
