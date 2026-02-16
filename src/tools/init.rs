//! Repository bootstrap helpers for Bacchus.
//!
//! `bacchus init` sets up jj (optionally), `.bacchus/`, task template, and
//! can create an initial epic.

use crate::epics;
use crate::tasks;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[derive(Debug, Clone, Serialize)]
pub struct InitOutput {
    pub success: bool,
    pub workspace_root: String,
    pub bacchus_dir: String,
    pub jj: InitJjStatus,
    pub tasks: InitTasksStatus,
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
