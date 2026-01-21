//! Archetype management for agent specialization
//!
//! Archetypes define specialized prompts and behaviors for different task types.
//! They are loaded from archetypes.yaml and selected based on task characteristics.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

use crate::tasks::{get_sqlite_task, SqliteTask, TasksError};

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

/// Select the best archetype for a task
pub fn select_archetype_for_task(task_id: &str) -> Result<ArchetypeSelection, ArchetypeError> {
    let task = get_sqlite_task(task_id)?;
    let archetypes = load_archetypes()?;

    select_archetype_for_task_data(&task, &archetypes)
}

/// Select archetype based on task data (for testing without DB)
pub fn select_archetype_for_task_data(
    task: &SqliteTask,
    archetypes: &ArchetypesFile,
) -> Result<ArchetypeSelection, ArchetypeError> {
    let mut best_score = 0;
    let mut best_name = "generic".to_string();
    let mut best_reasons: Vec<String> = vec![];

    // Combine title and description for keyword matching
    let text = format!(
        "{} {}",
        task.title.to_lowercase(),
        task.description.as_deref().unwrap_or("").to_lowercase()
    );

    // Get task type as string for matching
    let task_type_str = task.task_type.as_str();

    for (name, archetype) in &archetypes.archetypes {
        let mut score = 0;
        let mut reasons: Vec<String> = vec![];

        // Exact task type match is highest priority
        if name == task_type_str {
            score += 100;
            reasons.push(format!("Task type matches: {}", task_type_str));
        }

        // Keyword matching
        for keyword in &archetype.keywords {
            if text.contains(&keyword.to_lowercase()) {
                score += 10;
                reasons.push(format!("Keyword match: {}", keyword));
            }
        }

        // File pattern matching (if task has footprint info)
        // Note: This would require loading footprint data from task_footprints table
        // For now, we skip file pattern matching in the basic implementation

        if score > best_score {
            best_score = score;
            best_name = name.clone();
            best_reasons = reasons;
        }
    }

    // Get the selected archetype
    let archetype = archetypes.archetypes
        .get(&best_name)
        .cloned()
        .ok_or_else(|| ArchetypeError::NotFound(best_name.clone()))?;

    // If no match found, default to generic with a note
    if best_score == 0 {
        best_reasons.push("No specific match, using default".to_string());
    }

    Ok(ArchetypeSelection {
        archetype_name: best_name,
        archetype,
        score: best_score,
        reasons: best_reasons,
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
    use crate::tasks::SqliteTaskType;

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

    fn create_test_task(title: &str, task_type: SqliteTaskType) -> SqliteTask {
        SqliteTask {
            id: "TEST-001".to_string(),
            epic_id: "TEST".to_string(),
            title: title.to_string(),
            description: None,
            priority: 1,
            status: crate::tasks::SqliteTaskStatus::Open,
            task_type,
            claimed_by: None,
            claimed_at: None,
            ready_commit_id: None,
            release_commit_id: None,
            release_started_at: None,
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
        }
    }

    #[test]
    fn test_select_by_task_type() {
        let archetypes = create_test_archetypes();
        let task = create_test_task("Add login form", SqliteTaskType::Frontend);

        let selection = select_archetype_for_task_data(&task, &archetypes).unwrap();
        assert_eq!(selection.archetype_name, "frontend");
        assert!(selection.score >= 100); // Task type match
    }

    #[test]
    fn test_select_by_keywords() {
        let archetypes = create_test_archetypes();
        // Use Feature type which doesn't match any archetype name, so keywords will determine selection
        let task = create_test_task("Create React component for dashboard", SqliteTaskType::Feature);

        let selection = select_archetype_for_task_data(&task, &archetypes).unwrap();
        assert_eq!(selection.archetype_name, "frontend");
        assert!(selection.reasons.iter().any(|r| r.contains("component") || r.contains("react")));
    }

    #[test]
    fn test_select_fallback_to_generic() {
        let archetypes = create_test_archetypes();
        let task = create_test_task("Do something unrelated", SqliteTaskType::Generic);

        let selection = select_archetype_for_task_data(&task, &archetypes).unwrap();
        assert_eq!(selection.archetype_name, "generic");
    }
}
