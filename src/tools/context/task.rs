//! Task-specific context generation for agents
//!
//! Generates rich, type-aware context for agents working on tasks.

use crate::db::with_db;
use crate::tasks::{self, SqliteTaskType};
use std::path::Path;

/// Task context with all relevant information for an agent
pub struct TaskContext {
    pub task_id: String,
    pub title: String,
    pub description: Option<String>,
    pub task_type: SqliteTaskType,
    pub archetype: String,
    pub priority: i32,
    pub status: String,
    pub claimed_by: Option<String>,
    pub blocking_deps: Vec<String>, // Tasks that must be completed first
    pub unblocks: Vec<String>,      // Tasks that depend on this
    pub footprint_modifies: Vec<String>, // Symbols this task modifies
    pub footprint_creates: Vec<String>, // Files this task creates
    pub footprint_conflicts_in_progress: Vec<String>, // In-progress tasks with overlap
    pub risk_hints: Vec<String>,    // Computed coordination hints
}

/// Generate rich context for a task
pub fn generate_task_context(task_id: &str, _workspace_root: &Path) -> Result<String, String> {
    let ctx = get_task_context(task_id)?;
    Ok(format_task_context(&ctx))
}

/// Get structured task context
pub fn get_task_context(task_id: &str) -> Result<TaskContext, String> {
    let task = tasks::get_sqlite_task(task_id).map_err(|e| e.to_string())?;

    // Get dependencies (tasks that block this one)
    let blocking_deps: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT td.depends_on
             FROM task_dependencies td
             JOIN tasks dep ON dep.id = td.depends_on
             WHERE td.task_id = ?1
               AND dep.status != 'closed'
               AND dep.deleted_at IS NULL",
        )?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .map_err(|e: rusqlite::Error| e.to_string())?;

    // Get tasks that this one unblocks
    let unblocks: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT td.task_id
             FROM task_dependencies td
             JOIN tasks t ON t.id = td.task_id
             WHERE td.depends_on = ?1
               AND t.deleted_at IS NULL",
        )?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .map_err(|e: rusqlite::Error| e.to_string())?;

    // Get footprint info
    let (footprint_modifies, footprint_creates): (Vec<String>, Vec<String>) = with_db(|conn| {
        let mut modifies = Vec::new();
        let mut creates = Vec::new();

        let mut stmt = conn.prepare(
            "SELECT pattern_type, file_path, symbol, is_wildcard
             FROM task_footprints
             WHERE task_id = ?1",
        )?;

        let rows = stmt.query_map([task_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
            ))
        })?;

        for row in rows.flatten() {
            let (pattern_type, file_path, symbol, is_wildcard) = row;
            match pattern_type.as_str() {
                "modifies" => {
                    let pattern = if is_wildcard == 1 {
                        format!("{}::*", file_path)
                    } else {
                        format!("{}::{}", file_path, symbol)
                    };
                    modifies.push(pattern);
                }
                "creates" => {
                    creates.push(file_path);
                }
                _ => {}
            }
        }

        Ok((modifies, creates))
    })
    .map_err(|e: rusqlite::Error| e.to_string())?;

    let footprint_conflicts_in_progress: Vec<String> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT other.id
             FROM task_footprints fp1
             JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
               AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
             JOIN tasks other ON other.id = fp2.task_id
             WHERE fp1.task_id = ?1
               AND other.id != ?1
               AND other.status = 'in_progress'
               AND other.deleted_at IS NULL",
        )?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    })
    .map_err(|e: rusqlite::Error| e.to_string())?;

    let mut ctx = TaskContext {
        task_id: task.id,
        title: task.title,
        description: task.description,
        task_type: task.task_type,
        archetype: task.archetype,
        priority: task.priority,
        status: task.status.as_str().to_string(),
        claimed_by: task.claimed_by,
        blocking_deps,
        unblocks,
        footprint_modifies,
        footprint_creates,
        footprint_conflicts_in_progress,
        risk_hints: Vec::new(),
    };
    ctx.risk_hints = build_risk_hints(&ctx);

    Ok(ctx)
}

/// Format task context as markdown for agent consumption
pub fn format_task_context(ctx: &TaskContext) -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!("# Task: {} - {}\n\n", ctx.task_id, ctx.title));

    // Task metadata
    out.push_str("## Overview\n\n");
    out.push_str(&format!("- **Type**: {}\n", ctx.task_type.label()));
    out.push_str(&format!("- **Archetype**: {}\n", ctx.archetype));
    out.push_str(&format!("- **Priority**: {}\n", ctx.priority));
    out.push_str(&format!("- **Status**: {}\n", ctx.status));
    if let Some(ref agent) = ctx.claimed_by {
        out.push_str(&format!("- **Claimed by**: {}\n", agent));
    }
    out.push('\n');

    // Description
    if let Some(ref desc) = ctx.description {
        out.push_str("## Description\n\n");
        out.push_str(desc);
        out.push_str("\n\n");
    }

    // Dependencies
    if !ctx.blocking_deps.is_empty() {
        out.push_str("## Blocked By\n\n");
        out.push_str("These tasks must be completed before this one:\n\n");
        for dep in &ctx.blocking_deps {
            out.push_str(&format!("- `{}`\n", dep));
        }
        out.push('\n');
    }

    if !ctx.unblocks.is_empty() {
        out.push_str("## Unblocks\n\n");
        out.push_str("Completing this task will unblock:\n\n");
        for dep in &ctx.unblocks {
            out.push_str(&format!("- `{}`\n", dep));
        }
        out.push('\n');
    }

    // Footprint
    if !ctx.footprint_modifies.is_empty() || !ctx.footprint_creates.is_empty() {
        out.push_str("## Footprint\n\n");
        out.push_str("This task is expected to modify/create the following:\n\n");

        if !ctx.footprint_modifies.is_empty() {
            out.push_str("**Modifies:**\n");
            for pattern in &ctx.footprint_modifies {
                out.push_str(&format!("- `{}`\n", pattern));
            }
        }

        if !ctx.footprint_creates.is_empty() {
            out.push_str("\n**Creates:**\n");
            for path in &ctx.footprint_creates {
                out.push_str(&format!("- `{}`\n", path));
            }
        }
        out.push('\n');
    }

    if !ctx.footprint_conflicts_in_progress.is_empty() {
        out.push_str("## Active Footprint Collisions\n\n");
        out.push_str("These in-progress tasks overlap this footprint right now:\n\n");
        for task_id in &ctx.footprint_conflicts_in_progress {
            out.push_str(&format!("- `{}`\n", task_id));
        }
        out.push('\n');
    }

    if !ctx.risk_hints.is_empty() {
        out.push_str("## Risk Hints\n\n");
        for hint in &ctx.risk_hints {
            out.push_str(&format!("- {}\n", hint));
        }
        out.push('\n');
    }

    // Type-specific guidance
    out.push_str("## Guidance\n\n");
    out.push_str(&get_type_specific_guidance(ctx.task_type));
    out.push('\n');

    // Standard instructions
    out.push_str("## Workflow\n\n");
    out.push_str("1. Work in the isolated workspace: `.bacchus/workspaces/");
    out.push_str(&ctx.task_id);
    out.push_str("`\n");
    out.push_str("2. Changes are auto-snapshotted by jj\n");
    out.push_str("3. When complete, release with: `bacchus release ");
    out.push_str(&ctx.task_id);
    out.push_str(" --status done`\n");
    out.push_str("4. If blocked, release with: `bacchus release ");
    out.push_str(&ctx.task_id);
    out.push_str(" --status blocked`\n");

    out
}

/// Get task-type-specific guidance (PM workflow types only)
/// Domain-specific guidance is now provided by archetype prompts
fn get_type_specific_guidance(task_type: SqliteTaskType) -> String {
    match task_type {
        SqliteTaskType::BugFix => r#"**Bug Fix Guidance:**
- First, reproduce the bug to understand it
- Identify the root cause before making changes
- Write a failing test that demonstrates the bug
- Fix the issue with minimal changes
- Verify the fix doesn't introduce regressions
- Document what caused the bug in commit message"#
            .to_string(),

        SqliteTaskType::Feature => r#"**Feature Implementation Guidance:**
- Understand the requirements fully before coding
- Consider edge cases and error handling
- Follow existing code patterns and conventions
- Add appropriate tests for the new functionality
- Update documentation if needed
- Keep commits focused and incremental"#
            .to_string(),

        SqliteTaskType::Refactor => r#"**Refactoring Guidance:**
- Ensure tests pass before starting
- Make small, incremental changes
- Each commit should leave tests passing
- Don't mix refactoring with feature changes
- Verify behavior is preserved after changes
- Document why the refactoring improves the code"#
            .to_string(),

        SqliteTaskType::Test => r#"**Test Writing Guidance:**
- Focus on testing behavior, not implementation
- Include both happy path and edge cases
- Use descriptive test names that explain intent
- Keep tests independent and isolated
- Avoid testing implementation details
- Aim for meaningful coverage, not 100%"#
            .to_string(),

        SqliteTaskType::Docs => r#"**Documentation Guidance:**
- Write for the intended audience
- Use clear, concise language
- Include examples where helpful
- Keep formatting consistent
- Verify all links and references work
- Update related docs if needed"#
            .to_string(),

        SqliteTaskType::Infra => r#"**Infrastructure Guidance:**
- Test changes in a safe environment first
- Document configuration changes
- Consider rollback procedures
- Update monitoring/alerting if needed
- Keep security implications in mind
- Verify CI/CD pipelines work correctly"#
            .to_string(),

        SqliteTaskType::Generic => r#"**General Guidance:**
- Understand the scope before starting
- Follow existing code conventions
- Test your changes thoroughly
- Keep commits focused and well-documented
- Ask for clarification if requirements are unclear"#
            .to_string(),
    }
}

fn build_risk_hints(ctx: &TaskContext) -> Vec<String> {
    let mut hints = Vec::new();

    if ctx.footprint_modifies.is_empty() && ctx.footprint_creates.is_empty() {
        hints.push(
            "No footprint declared, so collision detection will not protect this task.".to_string(),
        );
    }

    let wildcard_count = ctx
        .footprint_modifies
        .iter()
        .filter(|p| p.ends_with("::*") || !p.contains("::"))
        .count();
    if wildcard_count > 0 {
        hints.push(format!(
            "Footprint includes {} file-level wildcard pattern(s), which increases merge and coordination risk.",
            wildcard_count
        ));
    }

    if !ctx.footprint_conflicts_in_progress.is_empty() {
        hints.push(format!(
            "{} overlapping task(s) are already in progress; coordinate sequencing before broad edits.",
            ctx.footprint_conflicts_in_progress.len()
        ));
    }

    if ctx.unblocks.len() >= 3 {
        hints.push(format!(
            "This task unblocks {} downstream tasks; regressions here will stall multiple agents.",
            ctx.unblocks.len()
        ));
    }

    if ctx.priority <= 2 {
        hints.push(
            "High-priority task: validate quickly and report blockers immediately to avoid queue starvation."
                .to_string(),
        );
    }

    if matches!(
        ctx.task_type,
        SqliteTaskType::BugFix | SqliteTaskType::Refactor
    ) {
        hints.push(
            "Behavior-sensitive task type: run targeted tests before release to catch regressions."
                .to_string(),
        );
    }

    let description_len = ctx
        .description
        .as_ref()
        .map(|d| d.trim().len())
        .unwrap_or_default();
    if description_len < 20 {
        hints.push(
            "Task description is brief; confirm scope and acceptance criteria before significant edits."
                .to_string(),
        );
    }

    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ctx() -> TaskContext {
        TaskContext {
            task_id: "T1".to_string(),
            title: "Demo".to_string(),
            description: Some("short".to_string()),
            task_type: SqliteTaskType::Refactor,
            archetype: "backend".to_string(),
            priority: 1,
            status: "open".to_string(),
            claimed_by: None,
            blocking_deps: vec![],
            unblocks: vec!["A".to_string(), "B".to_string(), "C".to_string()],
            footprint_modifies: vec!["src/lib.rs::*".to_string()],
            footprint_creates: vec![],
            footprint_conflicts_in_progress: vec!["T2".to_string()],
            risk_hints: vec![],
        }
    }

    #[test]
    fn test_build_risk_hints_includes_high_risk_signals() {
        let ctx = sample_ctx();
        let hints = build_risk_hints(&ctx);
        assert!(hints.iter().any(|h| h.contains("wildcard")));
        assert!(hints.iter().any(|h| h.contains("overlapping task")));
        assert!(hints.iter().any(|h| h.contains("High-priority")));
        assert!(hints.iter().any(|h| h.contains("Behavior-sensitive")));
    }

    #[test]
    fn test_build_risk_hints_flags_missing_footprint() {
        let mut ctx = sample_ctx();
        ctx.footprint_modifies.clear();
        ctx.unblocks.clear();
        ctx.priority = 5;
        ctx.description = Some("This is a sufficiently detailed description.".to_string());
        ctx.footprint_conflicts_in_progress.clear();
        let hints = build_risk_hints(&ctx);
        assert!(hints.iter().any(|h| h.contains("No footprint declared")));
    }
}
