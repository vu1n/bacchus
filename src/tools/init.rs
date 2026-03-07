//! Repository bootstrap helpers for Bacchus.
//!
//! `bacchus init` sets up jj (optionally), `.bacchus/`, task template,
//! project-level Claude Code skill + stop hook, and can create an initial epic.

use crate::epics;
use crate::tasks;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Embedded content (version-matched to binary)
const SKILL_MD: &str = include_str!("../../skills/bacchus/SKILL.md");
const ARCHETYPES_YAML: &str = include_str!("../../skills/bacchus/archetypes.yaml");
const CMD_WORKER: &str = include_str!("../../skills/bacchus/commands/bacchus-worker.md");
const CMD_ORCHESTRATOR: &str =
    include_str!("../../skills/bacchus/commands/bacchus-orchestrator.md");
const CMD_PLAN: &str = include_str!("../../skills/bacchus/commands/bacchus-plan.md");

/// The stop hook command (fail-open design)
const HOOK_CMD: &str = r#"bacchus session check 2>/dev/null || echo '{"decision":"approve"}'"#;

/// The activity reporter hook script (generated into .bacchus/hooks/)
const ACTIVITY_HOOK_SCRIPT: &str = r#"#!/bin/bash
# Async hook — reports worker activity to bacchus DB
INPUT=$(cat)
TOOL=$(echo "$INPUT" | jq -r '.tool_name // empty')

case "$TOOL" in
  Read|Glob|Grep) ACTIVITY="reading" ;;
  Edit|Write)     ACTIVITY="editing" ;;
  Bash)
    CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null)
    case "$CMD" in
      *test*|*spec*|*jest*|*vitest*|*pytest*) ACTIVITY="testing" ;;
      *build*|*compile*|*cargo*build*)        ACTIVITY="building" ;;
      *lint*|*clippy*|*biome*|*eslint*)       ACTIVITY="linting" ;;
      *)                                       ACTIVITY="running command" ;;
    esac
    ;;
  *) exit 0 ;;
esac

TASK_ID="${BACCHUS_TASK_ID:-}"
AGENT_ID="${BACCHUS_AGENT_ID:-}"
[ -z "$TASK_ID" ] && exit 0

bacchus activity "$TASK_ID" "$AGENT_ID" "$ACTIVITY" 2>/dev/null &

# Notify event server (if running) so orchestrator wakes from long-poll
# Prefer BACCHUS_EVENT_PORT env var (set by worker spawner) over port file discovery
PORT="${BACCHUS_EVENT_PORT:-}"
if [ -z "$PORT" ]; then
  RUN_ID="${BACCHUS_RUN_ID:-}"
  if [ -n "$RUN_ID" ]; then
    PORT_FILE="${CLAUDE_PROJECT_DIR:-.}/.bacchus/sessions/server_port_${RUN_ID}"
    [ -f "$PORT_FILE" ] && PORT=$(cat "$PORT_FILE" 2>/dev/null)
  fi
fi
[ -n "$PORT" ] && \
  curl -s --max-time 1 -X POST -H 'Content-Type: application/json' \
    -d "{\"type\":\"activity\",\"task_id\":\"$TASK_ID\",\"agent_id\":\"$AGENT_ID\",\"activity\":\"$ACTIVITY\"}" \
    "http://127.0.0.1:$PORT/event" >/dev/null 2>&1 &
"#;

/// The activity hook command reference for settings.json
const ACTIVITY_HOOK_CMD: &str = ".bacchus/hooks/report-activity.sh";

#[derive(Debug, Clone, Copy)]
pub struct InitOptions<'a> {
    pub skip_jj: bool,
    pub force_tasks: bool,
    pub epic_id: Option<&'a str>,
    pub epic_title: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InitJjStatus {
    pub attempted: bool,
    pub available: bool,
    pub already_initialized: bool,
    pub initialized: bool,
    pub mode: Option<String>,
    pub main_bookmark_created: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InitTasksStatus {
    pub path: String,
    pub created: bool,
    pub overwritten: bool,
    pub already_exists: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InitEpicStatus {
    pub id: String,
    pub title: String,
    pub created: bool,
    pub already_exists: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct InitClaudeStatus {
    pub skill_installed: bool,
    pub skill_already_exists: bool,
    pub hook_installed: bool,
    pub hook_already_exists: bool,
    pub archetypes_installed: bool,
    pub archetypes_already_exists: bool,
    pub commands_installed: Vec<String>,
    pub commands_already_exist: Vec<String>,
    pub claude_md_updated: bool,
    pub claude_md_already_has_pointer: bool,
    pub config_created: bool,
    pub config_already_exists: bool,
    pub db_ignore_added: bool,
    pub db_ignore_already_exists: bool,
    pub activity_hook_installed: bool,
    pub activity_hook_registered: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitOutput {
    pub success: bool,
    pub workspace_root: String,
    pub bacchus_dir: String,
    pub jj: InitJjStatus,
    pub tasks: InitTasksStatus,
    pub claude: InitClaudeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epic: Option<InitEpicStatus>,
    pub notes: Vec<String>,
}

fn command_stdout(workspace_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("jj")
        .args(args)
        .current_dir(workspace_root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

fn command_status(workspace_root: &Path, args: &[&str]) -> bool {
    Command::new("jj")
        .args(args)
        .current_dir(workspace_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn jj_available() -> bool {
    Command::new("jj")
        .arg("--version")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ensure_main_bookmark(workspace_root: &Path, status: &mut InitJjStatus) -> Result<(), String> {
    let bookmarks = command_stdout(
        workspace_root,
        &["bookmark", "list", "--template", "name ++ \"\\n\""],
    )?;
    let has_main = bookmarks.lines().any(|line| line.trim() == "main");
    if has_main {
        return Ok(());
    }
    command_stdout(workspace_root, &["bookmark", "create", "main", "-r", "@"])?;
    status.main_bookmark_created = true;
    Ok(())
}

fn ensure_jj(workspace_root: &Path, notes: &mut Vec<String>) -> Result<InitJjStatus, String> {
    let mut status = InitJjStatus {
        attempted: true,
        ..InitJjStatus::default()
    };

    if !jj_available() {
        notes.push(
            "jj is not installed; skipped jj bootstrap (install: https://martinvonz.github.io/jj/latest/install/)"
                .to_string(),
        );
        return Ok(status);
    }
    status.available = true;

    if command_status(workspace_root, &["root"]) {
        status.already_initialized = true;
        ensure_main_bookmark(workspace_root, &mut status)?;
        return Ok(status);
    }

    let has_git = workspace_root.join(".git").exists();
    if has_git {
        command_stdout(workspace_root, &["git", "init", "--colocate"])
            .map_err(|e| format!("failed to initialize jj in colocated mode: {}", e))?;
        status.mode = Some("colocated".to_string());
    } else {
        command_stdout(workspace_root, &["git", "init"])
            .map_err(|e| format!("failed to initialize jj git backend: {}", e))?;
        status.mode = Some("git".to_string());
    }
    status.initialized = true;
    ensure_main_bookmark(workspace_root, &mut status)?;

    let has_user_name = command_status(workspace_root, &["config", "get", "user.name"]);
    let has_user_email = command_status(workspace_root, &["config", "get", "user.email"]);
    if !has_user_name || !has_user_email {
        notes.push(
            "jj user identity is incomplete; set user.name and user.email in repo or global config"
                .to_string(),
        );
    }

    Ok(status)
}

fn ensure_tasks_template(workspace_root: &Path, force: bool) -> Result<InitTasksStatus, String> {
    let path = tasks::tasks_file_path(workspace_root);
    let mut status = InitTasksStatus {
        path: path.to_string_lossy().to_string(),
        ..InitTasksStatus::default()
    };

    if path.exists() {
        if force {
            fs::write(&path, tasks::generate_template()).map_err(|e| e.to_string())?;
            status.overwritten = true;
        } else {
            status.already_exists = true;
        }
        return Ok(status);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, tasks::generate_template()).map_err(|e| e.to_string())?;
    status.created = true;
    Ok(status)
}

fn ensure_epic(epic_id: &str, epic_title: Option<&str>) -> Result<InitEpicStatus, String> {
    let title = epic_title
        .map(|t| t.to_string())
        .unwrap_or_else(|| format!("{} Epic", epic_id));
    match epics::create_epic(epics::CreateEpicInput {
        id: epic_id.to_string(),
        title: title.clone(),
        description: Some("Initialized by bacchus init".to_string()),
        created_by: "human".to_string(),
    }) {
        Ok(_) => Ok(InitEpicStatus {
            id: epic_id.to_string(),
            title,
            created: true,
            already_exists: false,
        }),
        Err(epics::EpicsError::DuplicateEpic(_)) => Ok(InitEpicStatus {
            id: epic_id.to_string(),
            title,
            created: false,
            already_exists: true,
        }),
        Err(e) => Err(e.to_string()),
    }
}

/// Install project-level SKILL.md to .claude/skills/bacchus/
fn ensure_skill(workspace_root: &Path) -> Result<(bool, bool), String> {
    let skill_dir = workspace_root.join(".claude/skills/bacchus");
    let skill_path = skill_dir.join("SKILL.md");

    if skill_path.exists() {
        return Ok((false, true)); // (installed, already_exists)
    }

    fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    fs::write(&skill_path, SKILL_MD).map_err(|e| e.to_string())?;
    Ok((true, false))
}

/// Install project-level archetypes.yaml to .bacchus/
fn ensure_archetypes(workspace_root: &Path) -> Result<(bool, bool), String> {
    let path = workspace_root.join(".bacchus/archetypes.yaml");

    if path.exists() {
        return Ok((false, true));
    }

    fs::write(&path, ARCHETYPES_YAML).map_err(|e| e.to_string())?;
    Ok((true, false))
}

/// Install slash commands to .claude/commands/
fn ensure_commands(workspace_root: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let commands_dir = workspace_root.join(".claude/commands");
    fs::create_dir_all(&commands_dir).map_err(|e| e.to_string())?;

    let commands = [
        ("bacchus-worker.md", CMD_WORKER),
        ("bacchus-orchestrator.md", CMD_ORCHESTRATOR),
        ("bacchus-plan.md", CMD_PLAN),
    ];

    let mut installed = Vec::new();
    let mut already_exist = Vec::new();

    for (filename, content) in &commands {
        let path = commands_dir.join(filename);
        if path.exists() {
            already_exist.push(filename.to_string());
        } else {
            fs::write(&path, content).map_err(|e| e.to_string())?;
            installed.push(filename.to_string());
        }
    }

    Ok((installed, already_exist))
}

/// Upsert a hook entry into `.claude/settings.json` under the given event key.
///
/// Returns `Ok(true)` if the hook was added, `Ok(false)` if already present or settings.json
/// doesn't exist yet. Creates `.claude/` and `settings.json` when `create_if_missing` is true.
fn upsert_settings_hook(
    workspace_root: &Path,
    event_key: &str,
    command: &str,
    hook_entry: serde_json::Value,
    create_if_missing: bool,
) -> Result<bool, String> {
    let settings_path = workspace_root.join(".claude/settings.json");

    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
        let mut settings: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("invalid settings.json: {}", e))?;

        // Check if hook already exists
        let already = settings
            .get("hooks")
            .and_then(|h| h.get(event_key))
            .and_then(|s| s.as_array())
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .is_some_and(|hooks| {
                            hooks
                                .iter()
                                .any(|h| h.get("command").and_then(|c| c.as_str()) == Some(command))
                        })
                })
            });
        if already {
            return Ok(false);
        }

        // Append hook
        let hooks = settings
            .as_object_mut()
            .ok_or("settings.json is not an object")?
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}));
        let event_arr = hooks
            .as_object_mut()
            .ok_or("hooks is not an object")?
            .entry(event_key)
            .or_insert_with(|| serde_json::json!([]));
        event_arr
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{} is not an array", event_key))?
            .push(hook_entry);

        let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(&settings_path, out + "\n").map_err(|e| e.to_string())?;
        Ok(true)
    } else if create_if_missing {
        let claude_dir = workspace_root.join(".claude");
        fs::create_dir_all(&claude_dir).map_err(|e| e.to_string())?;

        let settings = serde_json::json!({
            "hooks": { event_key: [hook_entry] }
        });
        let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
        fs::write(&settings_path, out + "\n").map_err(|e| e.to_string())?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Install project-level stop hook in .claude/settings.json
fn ensure_hook(workspace_root: &Path) -> Result<(bool, bool), String> {
    let hook_entry = serde_json::json!({
        "hooks": [{"type": "command", "command": HOOK_CMD}]
    });
    let added = upsert_settings_hook(workspace_root, "Stop", HOOK_CMD, hook_entry, true)?;
    Ok(if added { (true, false) } else { (false, true) })
}

/// Install worker activity reporting hook script and register PostToolUse hook in settings.json.
///
/// Returns (script_installed, hook_registered) tuple.
fn ensure_worker_hooks(workspace_root: &Path) -> Result<(bool, bool), String> {
    let hooks_dir = workspace_root.join(".bacchus/hooks");
    let script_path = hooks_dir.join("report-activity.sh");

    // Install the shell script
    let script_installed = if !script_path.exists() {
        fs::create_dir_all(&hooks_dir).map_err(|e| e.to_string())?;
        fs::write(&script_path, ACTIVITY_HOOK_SCRIPT).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            fs::set_permissions(&script_path, perms).map_err(|e| e.to_string())?;
        }
        true
    } else {
        false
    };

    // Register PostToolUse hook in settings.json (don't create file — ensure_hook does that)
    let hook_entry = serde_json::json!({
        "hooks": [{"type": "command", "command": ACTIVITY_HOOK_CMD, "timeout": 5000}]
    });
    let hook_registered = upsert_settings_hook(
        workspace_root,
        "PostToolUse",
        ACTIVITY_HOOK_CMD,
        hook_entry,
        false,
    )?;

    Ok((script_installed, hook_registered))
}

/// Install project config to .bacchus/config.yaml with project-detected defaults (quality + worker)
fn ensure_config(workspace_root: &Path) -> Result<(bool, bool), String> {
    let path = workspace_root.join(".bacchus/config.yaml");

    if path.exists() {
        return Ok((false, true)); // (created, already_exists)
    }

    let content = crate::quality::generate_config(workspace_root);
    fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok((true, false))
}

/// Patterns that must be in the VCS ignore file to prevent jj/git from tracking runtime state.
/// If the DB is tracked, jj workspace operations (rebase, workspace create) will restore it
/// to an earlier committed state, reverting task statuses and losing runtime data.
const DB_IGNORE_PATTERNS: &[&str] = &[
    ".bacchus/bacchus.db",
    ".bacchus/bacchus.db-wal",
    ".bacchus/bacchus.db-shm",
    ".bacchus/sessions/",
    ".bacchus/logs/",
];

/// Marker comment we add to .gitignore so we can detect our section.
const IGNORE_MARKER: &str = "# bacchus runtime state";

/// Ensure the bacchus DB and session files are excluded from version control.
///
/// - Appends ignore patterns to `.gitignore` (creates if needed)
/// - If jj is available and files are tracked, runs `jj file untrack`
fn ensure_db_ignored(workspace_root: &Path) -> Result<(bool, bool), String> {
    let gitignore_path = workspace_root.join(".gitignore");

    // Check if patterns are already present
    let existing = fs::read_to_string(&gitignore_path).unwrap_or_default();
    if existing.contains(IGNORE_MARKER) {
        return Ok((false, true)); // (added, already_ignored)
    }

    // Append ignore patterns
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!("\n{}\n", IGNORE_MARKER));
    for pattern in DB_IGNORE_PATTERNS {
        content.push_str(&format!("{}\n", pattern));
    }

    fs::write(&gitignore_path, content).map_err(|e| e.to_string())?;

    // If jj is available, untrack already-tracked files (quiet — no stdout pollution)
    let jj_ok = Command::new("jj")
        .args(["root"])
        .current_dir(workspace_root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if jj_ok {
        for pattern in DB_IGNORE_PATTERNS {
            let full_path = workspace_root.join(pattern);
            if full_path.exists() {
                // Ignore errors — file may not be tracked
                let _ = Command::new("jj")
                    .args(["file", "untrack", pattern])
                    .current_dir(workspace_root)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .output();
            }
        }
    }

    Ok((true, false))
}

/// The marker we look for to detect an existing bacchus pointer in CLAUDE.md.
const CLAUDE_MD_MARKER: &str = ".bacchus/ORCHESTRATOR.md";

/// Bacchus pointer section appended to CLAUDE.md.
const CLAUDE_MD_SECTION: &str = r#"
## Bacchus
If `.bacchus/ORCHESTRATOR.md` exists, read it — you are the orchestrator. Follow its protocol.
"#;

/// Ensure CLAUDE.md has a pointer to the orchestrator breadcrumb.
/// - If CLAUDE.md doesn't exist: creates it with just the pointer section
/// - If it exists but has no bacchus section: appends the pointer section
/// - If it already has the bacchus section: no-op
fn ensure_claude_md(workspace_root: &Path) -> Result<(bool, bool), String> {
    let path = workspace_root.join("CLAUDE.md");

    if path.exists() {
        let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        if content.contains(CLAUDE_MD_MARKER) {
            return Ok((false, true)); // (updated, already_has_pointer)
        }
        // Append pointer section
        let mut new_content = content;
        if !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push_str(CLAUDE_MD_SECTION);
        fs::write(&path, new_content).map_err(|e| e.to_string())?;
        Ok((true, false))
    } else {
        fs::write(&path, CLAUDE_MD_SECTION.trim_start()).map_err(|e| e.to_string())?;
        Ok((true, false))
    }
}

pub fn init_workspace(
    workspace_root: &Path,
    options: InitOptions<'_>,
) -> Result<InitOutput, String> {
    let bacchus_dir: PathBuf = workspace_root.join(".bacchus");
    fs::create_dir_all(&bacchus_dir).map_err(|e| e.to_string())?;

    let mut notes = Vec::new();
    let jj = if options.skip_jj {
        notes.push("Skipped jj bootstrap (--skip-jj)".to_string());
        InitJjStatus::default()
    } else {
        ensure_jj(workspace_root, &mut notes)?
    };
    let tasks = ensure_tasks_template(workspace_root, options.force_tasks)?;

    // Install project-level Claude Code integration
    let mut claude = InitClaudeStatus::default();

    let (installed, exists) = ensure_skill(workspace_root)?;
    claude.skill_installed = installed;
    claude.skill_already_exists = exists;

    let (installed, exists) = ensure_hook(workspace_root)?;
    claude.hook_installed = installed;
    claude.hook_already_exists = exists;

    let (installed, exists) = ensure_archetypes(workspace_root)?;
    claude.archetypes_installed = installed;
    claude.archetypes_already_exists = exists;

    let (installed, already_exist) = ensure_commands(workspace_root)?;
    claude.commands_installed = installed;
    claude.commands_already_exist = already_exist;

    let (updated, already_has) = ensure_claude_md(workspace_root)?;
    claude.claude_md_updated = updated;
    claude.claude_md_already_has_pointer = already_has;

    let (created, exists) = ensure_config(workspace_root)?;
    claude.config_created = created;
    claude.config_already_exists = exists;

    let (added, exists) = ensure_db_ignored(workspace_root)?;
    claude.db_ignore_added = added;
    claude.db_ignore_already_exists = exists;

    let (script_installed, hook_registered) = ensure_worker_hooks(workspace_root)?;
    claude.activity_hook_installed = script_installed;
    claude.activity_hook_registered = hook_registered;

    let epic = match options.epic_id {
        Some(id) => Some(ensure_epic(id, options.epic_title)?),
        None => None,
    };

    if tasks.already_exists && !options.force_tasks {
        notes.push("tasks template already exists; use --force-tasks to overwrite".to_string());
    }

    Ok(InitOutput {
        success: true,
        workspace_root: workspace_root.to_string_lossy().to_string(),
        bacchus_dir: bacchus_dir.to_string_lossy().to_string(),
        jj,
        tasks,
        claude,
        epic,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{close_db, init_db};
    use tempfile::tempdir;

    #[test]
    fn test_init_creates_and_reuses_tasks_template() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap())).unwrap();

        let first = init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: false,
                epic_id: None,
                epic_title: None,
            },
        )
        .unwrap();
        assert!(first.tasks.created);
        assert!(dir.path().join(".bacchus/tasks.yaml").exists());

        let second = init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: false,
                epic_id: None,
                epic_title: None,
            },
        )
        .unwrap();
        assert!(second.tasks.already_exists);

        close_db();
    }

    #[test]
    fn test_init_force_overwrites_tasks_template() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap())).unwrap();

        let tasks_path = tasks::tasks_file_path(dir.path());
        if let Some(parent) = tasks_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&tasks_path, "version: 1\ntasks: []\n").unwrap();

        let out = init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: true,
                epic_id: None,
                epic_title: None,
            },
        )
        .unwrap();
        assert!(out.tasks.overwritten);
        let content = fs::read_to_string(&tasks_path).unwrap();
        assert!(content.contains("# Bacchus Task Configuration"));

        close_db();
    }

    #[test]
    fn test_init_adds_db_ignore_to_gitignore() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap())).unwrap();

        let out = init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: false,
                epic_id: None,
                epic_title: None,
            },
        )
        .unwrap();
        assert!(out.claude.db_ignore_added);
        assert!(!out.claude.db_ignore_already_exists);

        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(gitignore.contains("# bacchus runtime state"));
        assert!(gitignore.contains(".bacchus/bacchus.db"));
        assert!(gitignore.contains(".bacchus/bacchus.db-wal"));
        assert!(gitignore.contains(".bacchus/sessions/"));

        close_db();
    }

    #[test]
    fn test_init_db_ignore_idempotent() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap())).unwrap();

        // First init
        init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: false,
                epic_id: None,
                epic_title: None,
            },
        )
        .unwrap();

        let first_content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();

        // Second init
        let out = init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: false,
                epic_id: None,
                epic_title: None,
            },
        )
        .unwrap();
        assert!(!out.claude.db_ignore_added);
        assert!(out.claude.db_ignore_already_exists);

        let second_content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(
            first_content, second_content,
            "gitignore should not change on second init"
        );

        close_db();
    }

    #[test]
    fn test_init_db_ignore_appends_to_existing_gitignore() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap())).unwrap();

        // Pre-existing .gitignore
        fs::write(dir.path().join(".gitignore"), "node_modules/\ndist/\n").unwrap();

        let out = init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: false,
                epic_id: None,
                epic_title: None,
            },
        )
        .unwrap();
        assert!(out.claude.db_ignore_added);

        let gitignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(
            gitignore.starts_with("node_modules/"),
            "should preserve existing content"
        );
        assert!(
            gitignore.contains(".bacchus/bacchus.db"),
            "should add db ignore"
        );

        close_db();
    }

    #[test]
    fn test_init_epic_create_then_exists() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap())).unwrap();

        let first = init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: false,
                epic_id: Some("INIT-EPIC"),
                epic_title: Some("Init Epic"),
            },
        )
        .unwrap();
        assert!(first.epic.as_ref().is_some_and(|e| e.created));

        let second = init_workspace(
            dir.path(),
            InitOptions {
                skip_jj: true,
                force_tasks: false,
                epic_id: Some("INIT-EPIC"),
                epic_title: Some("Init Epic"),
            },
        )
        .unwrap();
        assert!(second.epic.as_ref().is_some_and(|e| e.already_exists));

        close_db();
    }
}
