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
use std::collections::HashMap;
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
    // Get declared footprint rules
    let rules: Vec<FootprintRule> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT pattern_type, file_path, symbol, is_wildcard
             FROM task_footprints
             WHERE task_id = ?1",
        )?;
        let rows = stmt
            .query_map([task_id], |row| {
                let pattern_type: String = row.get(0)?;
                let file_path: String = row.get(1)?;
                let symbol: String = row.get(2)?;
                let is_wildcard: i32 = row.get(3)?;
                Ok(FootprintRule {
                    pattern_type,
                    file_path: normalize_path(&file_path),
                    symbol: if is_wildcard == 1 || symbol.trim().is_empty() {
                        None
                    } else {
                        Some(symbol)
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap_or_default();

    if rules.is_empty() {
        return ReviewCheck {
            name: "Footprint compliance".to_string(),
            passed: true,
            message: "No footprint declared (all changes allowed)".to_string(),
        };
    }

    let changed_files = match collect_changed_files_with_hunks(workspace_path) {
        Ok(files) => files,
        Err(e) => {
            return ReviewCheck {
                name: "Footprint compliance".to_string(),
                passed: false,
                message: e,
            };
        }
    };

    let mut violations = Vec::new();
    let mut symbol_cache: HashMap<(String, String), Vec<LineRange>> = HashMap::new();

    for changed in &changed_files {
        let file_rules: Vec<&FootprintRule> = rules
            .iter()
            .filter(|r| r.file_path == changed.path)
            .collect();

        if file_rules.is_empty() {
            violations.push(format!("{} (not declared in footprint)", changed.path));
            continue;
        }

        let allows_create = file_rules.iter().any(|r| r.pattern_type == "creates");
        let allows_file_wildcard = file_rules
            .iter()
            .any(|r| r.pattern_type == "modifies" && r.symbol.is_none());

        if changed.is_new && allows_create {
            continue;
        }

        if allows_file_wildcard {
            continue;
        }

        let symbol_rules: Vec<&str> = file_rules
            .iter()
            .filter(|r| r.pattern_type == "modifies")
            .filter_map(|r| r.symbol.as_deref())
            .collect();

        if symbol_rules.is_empty() {
            if allows_create && !changed.is_new {
                if changed.is_deleted {
                    violations.push(format!(
                        "{} (listed under creates, but file was deleted; declare modifies for deletions)",
                        changed.path
                    ));
                } else {
                    violations.push(format!(
                        "{} (listed under creates, but modified an existing file; declare modifies)",
                        changed.path
                    ));
                }
            }
            continue;
        }

        if changed.hunks.is_empty() {
            violations.push(format!(
                "{} (non-line diff cannot be validated against symbol-level footprint; declare {}::* if intended)",
                changed.path, changed.path
            ));
            continue;
        }

        let mut allowed_ranges = Vec::new();
        let mut missing_symbols = Vec::new();
        let mut symbol_lookup_failed = false;

        for symbol in symbol_rules {
            let key = (changed.path.clone(), symbol.to_string());
            if let Some(cached) = symbol_cache.get(&key) {
                if cached.is_empty() {
                    missing_symbols.push(symbol.to_string());
                } else {
                    allowed_ranges.extend(cached.iter().copied());
                }
                continue;
            }

            match load_symbol_ranges(&changed.path, symbol) {
                Ok(ranges) => {
                    if ranges.is_empty() {
                        missing_symbols.push(symbol.to_string());
                    } else {
                        allowed_ranges.extend(ranges.iter().copied());
                    }
                    symbol_cache.insert(key, ranges);
                }
                Err(e) => {
                    symbol_lookup_failed = true;
                    violations.push(format!(
                        "{} (failed loading symbol '{}' from index: {})",
                        changed.path, symbol, e
                    ));
                }
            }
        }

        if symbol_lookup_failed {
            continue;
        }

        if !missing_symbols.is_empty() {
            violations.push(format!(
                "{} (declared symbol(s) missing from index: {}; run `bacchus index {}`)",
                changed.path,
                missing_symbols.join(", "),
                changed.path
            ));
            continue;
        }

        let mut out_of_bounds = Vec::new();
        for hunk in &changed.hunks {
            if !hunk_overlaps_any(hunk, &allowed_ranges) {
                out_of_bounds.push(render_hunk(hunk));
            }
        }

        if !out_of_bounds.is_empty() {
            violations.push(format!(
                "{} (changed outside declared symbols at {})",
                changed.path,
                out_of_bounds.join(", ")
            ));
        }
    }

    if violations.is_empty() {
        ReviewCheck {
            name: "Footprint compliance".to_string(),
            passed: true,
            message: format!(
                "All {} changed file(s) comply with declared footprint",
                changed_files.len()
            ),
        }
    } else {
        ReviewCheck {
            name: "Footprint compliance".to_string(),
            passed: false,
            message: format!(
                "{} footprint violation(s): {}",
                violations.len(),
                violations.join(" | ")
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct FootprintRule {
    pattern_type: String,
    file_path: String,
    symbol: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct LineRange {
    start: u32,
    end: u32,
}

impl LineRange {
    fn overlaps(&self, other: &LineRange) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

#[derive(Debug, Clone, Copy)]
struct DiffHunk {
    old: Option<LineRange>,
    new: Option<LineRange>,
}

#[derive(Debug, Clone, Default)]
struct ChangedFile {
    path: String,
    is_new: bool,
    is_deleted: bool,
    hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, Default)]
struct RawChangedFile {
    old_path: String,
    new_path: String,
    is_new: bool,
    is_deleted: bool,
    hunks: Vec<DiffHunk>,
}

fn normalize_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_start_matches("a/")
        .trim_start_matches("b/")
        .to_string()
}

fn collect_changed_files_with_hunks(workspace_path: &Path) -> Result<Vec<ChangedFile>, String> {
    let output = Command::new("jj")
        .args(["diff", "-r", "@", "--git"])
        .current_dir(workspace_path)
        .output()
        .map_err(|e| format!("Failed to run jj diff --git: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to get changed files: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_jj_git_diff(&text))
}

fn parse_jj_git_diff(diff: &str) -> Vec<ChangedFile> {
    let mut out = Vec::new();
    let mut current: Option<RawChangedFile> = None;

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            if let Some(raw) = current.take() {
                if let Some(file) = finalize_changed_file(raw) {
                    out.push(file);
                }
            }

            let parts: Vec<&str> = line.split_whitespace().collect();
            let old_path = parts
                .get(2)
                .map(|p| p.trim_start_matches("a/").to_string())
                .unwrap_or_default();
            let new_path = parts
                .get(3)
                .map(|p| p.trim_start_matches("b/").to_string())
                .unwrap_or_default();
            current = Some(RawChangedFile {
                old_path,
                new_path,
                ..RawChangedFile::default()
            });
            continue;
        }

        let Some(file) = current.as_mut() else {
            continue;
        };

        if let Some(path) = line.strip_prefix("rename from ") {
            file.old_path = path.to_string();
            continue;
        }
        if let Some(path) = line.strip_prefix("rename to ") {
            file.new_path = path.to_string();
            continue;
        }
        if line.starts_with("new file mode ") {
            file.is_new = true;
            continue;
        }
        if line.starts_with("deleted file mode ") {
            file.is_deleted = true;
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some(hunk) = parse_hunk_header(line) {
                file.hunks.push(hunk);
            }
        }
    }

    if let Some(raw) = current.take() {
        if let Some(file) = finalize_changed_file(raw) {
            out.push(file);
        }
    }

    out
}

fn finalize_changed_file(raw: RawChangedFile) -> Option<ChangedFile> {
    let old_path = normalize_path(&raw.old_path);
    let new_path = normalize_path(&raw.new_path);

    let path = if !new_path.is_empty() && new_path != "/dev/null" {
        new_path
    } else {
        old_path
    };

    if path.is_empty() || path == "/dev/null" {
        return None;
    }

    Some(ChangedFile {
        path,
        is_new: raw.is_new || raw.old_path.trim() == "/dev/null",
        is_deleted: raw.is_deleted || raw.new_path.trim() == "/dev/null",
        hunks: raw.hunks,
    })
}

fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let body = line.strip_prefix("@@ ")?;
    let end = body.find(" @@")?;
    let header = &body[..end];
    let mut parts = header.split_whitespace();
    let old_spec = parts.next()?;
    let new_spec = parts.next()?;
    Some(DiffHunk {
        old: parse_hunk_side(old_spec, '-'),
        new: parse_hunk_side(new_spec, '+'),
    })
}

fn parse_hunk_side(spec: &str, prefix: char) -> Option<LineRange> {
    let values = spec.strip_prefix(prefix)?;
    let (start_raw, count) = match values.split_once(',') {
        Some((start, count)) => (start, count.parse::<u32>().ok()?),
        None => (values, 1),
    };

    if count == 0 {
        return None;
    }

    let start: u32 = start_raw.parse().ok()?;
    let start = start.max(1);
    Some(LineRange {
        start,
        end: start.saturating_add(count).saturating_sub(1),
    })
}

fn hunk_overlaps_any(hunk: &DiffHunk, allowed: &[LineRange]) -> bool {
    allowed.iter().any(|range| {
        hunk.old.is_some_and(|old| old.overlaps(range))
            || hunk.new.is_some_and(|new| new.overlaps(range))
    })
}

fn render_hunk(hunk: &DiffHunk) -> String {
    if let Some(new) = hunk.new {
        return format!("new:{}-{}", new.start, new.end);
    }
    if let Some(old) = hunk.old {
        return format!("old:{}-{}", old.start, old.end);
    }
    "unknown".to_string()
}

fn load_symbol_ranges(file_path: &str, symbol: &str) -> Result<Vec<LineRange>, String> {
    let fq_name = format!("{}::{}", file_path, symbol);
    crate::db::with_db_str(|conn| {
        let mut stmt = conn.prepare(
            "SELECT span_start_line, span_end_line
             FROM symbols
             WHERE file = ?1
               AND fq_name = ?2",
        )?;
        let rows = stmt
            .query_map([file_path, fq_name.as_str()], |row| {
                Ok(LineRange {
                    start: row.get::<_, u32>(0)?,
                    end: row.get::<_, u32>(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hunk_header_handles_single_line_format() {
        let hunk = parse_hunk_header("@@ -12 +20 @@ fn demo").unwrap();
        assert_eq!(hunk.old.unwrap().start, 12);
        assert_eq!(hunk.old.unwrap().end, 12);
        assert_eq!(hunk.new.unwrap().start, 20);
        assert_eq!(hunk.new.unwrap().end, 20);
    }

    #[test]
    fn test_parse_jj_git_diff_tracks_new_and_deleted_files() {
        let diff = r#"diff --git a/src/new.rs b/src/new.rs
new file mode 100644
@@ -0,0 +1,3 @@
+fn a() {}
diff --git a/src/old.rs b/src/old.rs
deleted file mode 100644
@@ -5,2 +0,0 @@
-line
"#;

        let files = parse_jj_git_diff(diff);
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.path == "src/new.rs" && f.is_new));
        assert!(files.iter().any(|f| f.path == "src/old.rs" && f.is_deleted));
    }

    #[test]
    fn test_hunk_overlap_accepts_old_or_new_range() {
        let allowed = vec![LineRange { start: 10, end: 20 }];
        let within_old = DiffHunk {
            old: Some(LineRange { start: 15, end: 16 }),
            new: Some(LineRange { start: 50, end: 51 }),
        };
        let within_new = DiffHunk {
            old: Some(LineRange { start: 1, end: 2 }),
            new: Some(LineRange { start: 11, end: 12 }),
        };
        let outside = DiffHunk {
            old: Some(LineRange { start: 1, end: 2 }),
            new: Some(LineRange { start: 30, end: 40 }),
        };

        assert!(hunk_overlaps_any(&within_old, &allowed));
        assert!(hunk_overlaps_any(&within_new, &allowed));
        assert!(!hunk_overlaps_any(&outside, &allowed));
    }
}
