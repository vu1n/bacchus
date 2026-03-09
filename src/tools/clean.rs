//! Remove bacchus hooks and configuration from the repository.

use crate::config::find_workspace_root;
use std::fs;
use std::path::Path;

use super::init::{ACTIVITY_HOOK_CMD, CMD_ORCHESTRATOR, CMD_PLAN, CMD_WORKER, HOOK_CMD};
use super::session::stop_session;

/// Remove bacchus hooks and optionally all configuration.
///
/// - `remove_all`: If true, remove .bacchus/ directory (including DB)
/// - `dry_run`: If true, only show what would be removed without actually removing
pub fn clean_workspace(remove_all: bool, dry_run: bool) -> Result<String, String> {
    let root = find_workspace_root().ok_or("No workspace root found")?;

    // Stop any active session first (no-op if no session)
    if !dry_run {
        let _ = stop_session();
    }

    let mut result = CleanResult::default();

    // 1. Remove hooks from .claude/settings.json
    let settings_path = root.join(".claude/settings.json");
    match remove_hooks(&settings_path, dry_run) {
        Ok(removed) => {
            if removed {
                result.add_removed("hooks from .claude/settings.json");
            }
        }
        Err(e) => result.add_error("Failed to remove hooks", &e),
    }

    // 2. Remove .claude/skills/bacchus/
    let skill_dir = root.join(".claude/skills/bacchus");
    remove_path(&skill_dir, ".claude/skills/bacchus/", dry_run, &mut result);

    // 3. Remove .claude/commands/bacchus-*.md
    let commands_dir = root.join(".claude/commands");
    if commands_dir.is_dir() {
        let command_files = [
            (CMD_WORKER, "bacchus-worker.md"),
            (CMD_ORCHESTRATOR, "bacchus-orchestrator.md"),
            (CMD_PLAN, "bacchus-plan.md"),
        ];
        for (_content, filename) in command_files {
            let cmd_path = commands_dir.join(filename);
            remove_path(
                &cmd_path,
                &format!(".claude/commands/{}", filename),
                dry_run,
                &mut result,
            );
        }
    }

    // 4. Optionally remove .bacchus/ directory
    if remove_all {
        let bacchus_dir = root.join(".bacchus");
        remove_path(
            &bacchus_dir,
            ".bacchus/ (including database)",
            dry_run,
            &mut result,
        );
    }

    Ok(result.format(remove_all))
}

/// Remove bacchus hooks from .claude/settings.json.
///
/// Returns Ok(true) if hooks were removed/found, Ok(false) if none were found.
fn remove_hooks(settings_path: &Path, dry_run: bool) -> Result<bool, String> {
    let content = fs::read_to_string(settings_path).map_err(|e| e.to_string())?;
    let mut settings: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("invalid settings.json: {}", e))?;

    let hooks_obj = settings
        .as_object_mut()
        .and_then(|s| s.get_mut("hooks"))
        .and_then(|h| h.as_object_mut());

    let hooks_obj = match hooks_obj {
        Some(h) => h,
        None => return Ok(false),
    };

    let mut removed = false;

    // Remove Stop hook
    removed |= remove_hook_by_command(hooks_obj, "Stop", HOOK_CMD);

    // Remove PostToolUse hook
    removed |= remove_hook_by_command(hooks_obj, "PostToolUse", ACTIVITY_HOOK_CMD);

    if removed && !dry_run {
        let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(settings_path, out + "\n").map_err(|e| e.to_string())?;
    }

    Ok(removed)
}

/// Generic helper to remove hooks by command pattern.
fn remove_hook_by_command(
    hooks_obj: &mut serde_json::Map<String, serde_json::Value>,
    hook_type: &str,
    command_pattern: &str,
) -> bool {
    // Extract the hooks array if it exists
    let hooks_array = hooks_obj.get(hook_type).and_then(|v| v.as_array()).cloned();
    let hooks_array = match hooks_array {
        Some(arr) => arr,
        None => return false,
    };

    // Filter out matching hooks
    let filtered: Vec<_> = hooks_array
        .iter()
        .filter(|entry| {
            !entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|hooks| {
                    hooks.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c.contains(command_pattern))
                            .unwrap_or(false)
                    })
                })
        })
        .cloned()
        .collect();

    let removed = filtered.len() != hooks_array.len();

    if removed {
        if filtered.is_empty() {
            hooks_obj.remove(hook_type);
        } else {
            hooks_obj.insert(hook_type.to_string(), serde_json::json!(filtered));
        }
    }

    removed
}

/// Helper to remove a file or directory, tracking results.
fn remove_path(path: &Path, display_name: &str, dry_run: bool, result: &mut CleanResult) {
    // Try remove directly and handle error - avoids TOCTOU race condition
    let removal_result = if dry_run {
        // In dry run, check if path exists for display
        if path.exists() {
            result.add_removed(display_name);
        }
        return;
    } else {
        // Try the appropriate removal based on path type
        if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    };

    // Handle error if operation failed (and not dry run)
    if let Err(e) = removal_result {
        // Ignore "not found" errors - already gone or never existed
        if e.kind() != std::io::ErrorKind::NotFound {
            result.add_error(display_name, &e.to_string());
        }
    } else {
        result.add_removed(display_name);
    }
}

/// Tracks removal results with efficient string building.
#[derive(Default)]
struct CleanResult {
    removed: Vec<String>,
    errors: Vec<String>,
}

impl CleanResult {
    fn add_removed(&mut self, item: &str) {
        self.removed.push(item.to_string());
    }

    fn add_error(&mut self, context: &str, error: &str) {
        self.errors.push(format!("{}: {}", context, error));
    }

    fn format(&self, remove_all: bool) -> String {
        if self.removed.is_empty() && self.errors.is_empty() {
            return "No bacchus configuration found to remove.".to_string();
        }

        let mut result = String::with_capacity(512);

        if !self.removed.is_empty() {
            result.push_str("Removed:\n");
            for item in &self.removed {
                result.push_str("  - ");
                result.push_str(item);
                result.push('\n');
            }
        }

        if !self.errors.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("Errors:\n");
            for error in &self.errors {
                result.push_str("  - ");
                result.push_str(error);
                result.push('\n');
            }
        }

        if remove_all {
            result.push_str("\nNote: Run 'bacchus init' to reinstall bacchus in this repository.");
        } else {
            result.push_str(
                "\nNote: Database and workspaces preserved. Use --remove-all to delete everything.",
            );
        }

        result
    }
}
