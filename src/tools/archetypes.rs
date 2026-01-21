//! Archetype management for agent specialization
//!
//! Archetypes define specialized prompts and behaviors for different task types.
//! They are loaded from archetypes.yaml and selected based on task characteristics.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

use crate::tasks::{get_sqlite_task, TasksError};

/// Default archetypes.yaml location (relative to skill dir)
const DEFAULT_ARCHETYPES_FILENAME: &str = "archetypes.yaml";

/// Skill directory under ~/.claude/skills/bacchus/
const SKILL_DIR: &str = ".claude/skills/bacchus";

#[derive(Error, Debug)]
pub enum ArchetypeError {
    #[error("Archetype not found: {0}")]
    NotFound(String),
    #[error("Failed to read archetypes file: {0}")]
    ReadError(String),
    #[error("Failed to parse archetypes file: {0}")]
    ParseError(String),
    #[error("Task error: {0}")]
    TaskError(#[from] TasksError),
    #[error("No archetypes file found")]
    NoFile,
}

/// A single archetype definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Archetype {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub file_patterns: Vec<String>,
    pub prompt: String,
}

/// The archetypes.yaml file structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchetypesFile {
    pub version: i32,
    pub archetypes: HashMap<String, Archetype>,
}

/// Result of archetype selection
#[derive(Debug, Clone, Serialize)]
pub struct ArchetypeSelection {
    pub archetype_name: String,
    pub archetype: Archetype,
    pub score: i32,
    pub reasons: Vec<String>,
}

/// Find the archetypes.yaml file
/// Searches in order:
/// 1. .bacchus/archetypes.yaml (project-level override)
/// 2. ~/.claude/skills/bacchus/archetypes.yaml (user skill dir)
pub fn find_archetypes_file() -> Option<std::path::PathBuf> {
    // Check project-level override first
    let project_path = Path::new(".bacchus").join(DEFAULT_ARCHETYPES_FILENAME);
    if project_path.exists() {
        return Some(project_path);
    }

    // Check user skill directory
    if let Ok(home) = std::env::var("HOME") {
        let skill_path = Path::new(&home).join(SKILL_DIR).join(DEFAULT_ARCHETYPES_FILENAME);
        if skill_path.exists() {
            return Some(skill_path);
        }
    }

    None
}

/// Load archetypes from file
pub fn load_archetypes() -> Result<ArchetypesFile, ArchetypeError> {
    let path = find_archetypes_file().ok_or(ArchetypeError::NoFile)?;
    load_archetypes_from_path(&path)
}

/// Load archetypes from a specific path
pub fn load_archetypes_from_path(path: &Path) -> Result<ArchetypesFile, ArchetypeError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ArchetypeError::ReadError(e.to_string()))?;

    let archetypes: ArchetypesFile = serde_yaml::from_str(&content)
        .map_err(|e| ArchetypeError::ParseError(e.to_string()))?;

    Ok(archetypes)
}

/// Get a specific archetype by name
pub fn get_archetype(name: &str) -> Result<Archetype, ArchetypeError> {
    let archetypes = load_archetypes()?;
    archetypes.archetypes
        .get(name)
        .cloned()
        .ok_or_else(|| ArchetypeError::NotFound(name.to_string()))
}

/// List all available archetypes
pub fn list_archetypes() -> Result<Vec<(String, Archetype)>, ArchetypeError> {
    let archetypes = load_archetypes()?;
    let mut list: Vec<_> = archetypes.archetypes.into_iter().collect();
    // Sort by name for consistent output
    list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(list)
}

/// Select the archetype for a task based on its archetype field
pub fn select_archetype_for_task(task_id: &str) -> Result<ArchetypeSelection, ArchetypeError> {
    let task = get_sqlite_task(task_id)?;

    // Direct lookup by task's archetype field
    let archetype_name = &task.archetype;
    let archetype = get_archetype(archetype_name)?;

    Ok(ArchetypeSelection {
        archetype_name: archetype_name.clone(),
        archetype,
        score: 100,
        reasons: vec!["Planner assigned archetype".to_string()],
    })
}

/// CLI: List all archetypes
pub fn cmd_list_archetypes() -> String {
    match list_archetypes() {
        Ok(archetypes) => {
            let output: Vec<serde_json::Value> = archetypes
                .iter()
                .map(|(name, arch)| {
                    serde_json::json!({
                        "name": name,
                        "display_name": arch.name,
                        "description": arch.description,
                        "keywords": arch.keywords,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "[]".to_string())
        }
        Err(e) => {
            serde_json::json!({
                "error": e.to_string()
            }).to_string()
        }
    }
}

/// CLI: Show archetype details
pub fn cmd_show_archetype(name: &str) -> String {
    match get_archetype(name) {
        Ok(archetype) => {
            serde_json::to_string_pretty(&serde_json::json!({
                "name": name,
                "display_name": archetype.name,
                "description": archetype.description,
                "keywords": archetype.keywords,
                "file_patterns": archetype.file_patterns,
            })).unwrap_or_else(|_| "{}".to_string())
        }
        Err(e) => {
            serde_json::json!({
                "error": e.to_string()
            }).to_string()
        }
    }
}

/// CLI: Get archetype prompt
pub fn cmd_archetype_prompt(name: &str) -> String {
    match get_archetype(name) {
        Ok(archetype) => archetype.prompt,
        Err(e) => format!("Error: {}", e),
    }
}

/// CLI: Select archetype for task
pub fn cmd_select_archetype(task_id: &str) -> String {
    match select_archetype_for_task(task_id) {
        Ok(selection) => {
            serde_json::to_string_pretty(&serde_json::json!({
                "task_id": task_id,
                "archetype": selection.archetype_name,
                "display_name": selection.archetype.name,
                "description": selection.archetype.description,
                "score": selection.score,
                "reasons": selection.reasons,
                "prompt": selection.archetype.prompt,
            })).unwrap_or_else(|_| "{}".to_string())
        }
        Err(e) => {
            serde_json::json!({
                "error": e.to_string()
            }).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_archetypes() -> ArchetypesFile {
        let mut archetypes = HashMap::new();

        archetypes.insert("frontend".to_string(), Archetype {
            name: "Frontend Design".to_string(),
            description: "UI/UX specialist".to_string(),
            keywords: vec!["component".to_string(), "react".to_string(), "css".to_string()],
            file_patterns: vec!["*.tsx".to_string()],
            prompt: "You are a frontend agent.".to_string(),
        });

        archetypes.insert("backend".to_string(), Archetype {
            name: "Backend API".to_string(),
            description: "API specialist".to_string(),
            keywords: vec!["api".to_string(), "endpoint".to_string(), "handler".to_string()],
            file_patterns: vec!["**/api/**".to_string()],
            prompt: "You are a backend agent.".to_string(),
        });

        archetypes.insert("generic".to_string(), Archetype {
            name: "Generic".to_string(),
            description: "General purpose".to_string(),
            keywords: vec![],
            file_patterns: vec![],
            prompt: "You are a general agent.".to_string(),
        });

        ArchetypesFile {
            version: 1,
            archetypes,
        }
    }

    #[test]
    fn test_load_archetypes_from_file() {
        let archetypes = create_test_archetypes();
        assert!(archetypes.archetypes.contains_key("frontend"));
        assert!(archetypes.archetypes.contains_key("backend"));
        assert!(archetypes.archetypes.contains_key("generic"));
    }

    #[test]
    fn test_archetype_has_prompt() {
        let archetypes = create_test_archetypes();
        let frontend = archetypes.archetypes.get("frontend").unwrap();
        assert!(!frontend.prompt.is_empty());
    }
}
