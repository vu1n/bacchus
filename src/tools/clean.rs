//! Remove bacchus hooks and configuration from the repository.

use crate::config::find_workspace_root;
use std::fs;
use std::path::Path;

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

    let mut removed_items: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // 1. Remove hooks from .claude/settings.json
    let settings_path = root.join(".claude/settings.json");
    if settings_path.exists() {
        match remove_hooks(&settings_path, dry_run) {
            Ok(removed) => {
                if removed {
                    removed_items.push("hooks from .claude/settings.json".to_string());
                }
            }
            Err(e) => errors.push(format!("Failed to remove hooks: {}", e)),
        }
    }

    // 2. Remove .claude/skills/bacchus/
    let skill_dir = root.join(".claude/skills/bacchus");
    if skill_dir.exists() {
        if dry_run {
            removed_items.push(".claude/skills/bacchus/".to_string());
        } else {
            match fs::remove_dir_all(&skill_dir) {
                Ok(_) => removed_items.push(".claude/skills/bacchus/".to_string()),
                Err(e) => errors.push(format!("Failed to remove skills: {}", e)),
            }
        }
    }

    // 3. Remove .claude/commands/bacchus-*.md
    let commands_dir = root.join(".claude/commands");
    if commands_dir.exists() {
        let command_files = [
            "bacchus-worker.md",
            "bacchus-orchestrator.md",
            "bacchus-plan.md",
        ];
        for cmd in command_files {
            let cmd_path = commands_dir.join(cmd);
            if cmd_path.exists() {
                if dry_run {
                    removed_items.push(format!(".claude/commands/{}", cmd));
                } else {
                    match fs::remove_file(&cmd_path) {
                        Ok(_) => removed_items.push(format!(".claude/commands/{}", cmd)),
                        Err(e) => errors.push(format!("Failed to remove {}: {}", cmd, e)),
                    }
                }
            }
        }
    }

    // 4. Optionally remove .bacchus/ directory
    if remove_all {
        let bacchus_dir = root.join(".bacchus");
        if bacchus_dir.exists() {
            if dry_run {
                removed_items.push(".bacchus/ (including database)".to_string());
            } else {
                match fs::remove_dir_all(&bacchus_dir) {
                    Ok(_) => removed_items.push(".bacchus/ (including database)".to_string()),
                    Err(e) => errors.push(format!("Failed to remove .bacchus/: {}", e)),
                }
            }
        }
    }

    // Build result message
    if removed_items.is_empty() && errors.is_empty() {
        return Ok("No bacchus configuration found to remove.".to_string());
    }

    let mut result = String::new();
    if !removed_items.is_empty() {
        result.push_str("Removed:\n");
        for item in &removed_items {
            result.push_str(&format!("  - {}\n", item));
        }
    }

    if !errors.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("Errors:\n");
        for error in &errors {
            result.push_str(&format!("  - {}\n", error));
        }
    }

    if remove_all {
        result.push_str("\nNote: Run 'bacchus init' to reinstall bacchus in this repository.");
    } else {
        result.push_str(
            "\nNote: Database and workspaces preserved. Use --remove-all to delete everything.",
        );
    }

    Ok(result)
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
    if let Some(stop_hooks) = hooks_obj.get_mut("Stop").and_then(|v| v.as_array_mut()) {
        stop_hooks.retain(|entry| {
            !entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|hooks| {
                    hooks.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c.contains("bacchus session check"))
                            .unwrap_or(false)
                    })
                })
        });
        if stop_hooks.is_empty() {
            hooks_obj.remove("Stop");
        }
        removed = true;
    }

    // Remove PostToolUse hook
    if let Some(post_hooks) = hooks_obj
        .get_mut("PostToolUse")
        .and_then(|v| v.as_array_mut())
    {
        post_hooks.retain(|entry| {
            !entry
                .get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|hooks| {
                    hooks.iter().any(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| c.contains(".bacchus/hooks/report-activity.sh"))
                            .unwrap_or(false)
                    })
                })
        });
        if post_hooks.is_empty() {
            hooks_obj.remove("PostToolUse");
        }
        removed = true;
    }

    if removed {
        if dry_run {
            return Ok(true);
        }
        let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(settings_path, out + "\n").map_err(|e| e.to_string())?;
    }

    Ok(removed)
}
