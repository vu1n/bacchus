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
    pub priority: i32,
    pub status: String,
    pub claimed_by: Option<String>,
    pub blocking_deps: Vec<String>,     // Tasks that must be completed first
    pub unblocks: Vec<String>,          // Tasks that depend on this
    pub footprint_modifies: Vec<String>, // Symbols this task modifies
    pub footprint_creates: Vec<String>,  // Files this task creates
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

    Ok(TaskContext {
        task_id: task.id,
        title: task.title,
        description: task.description,
        task_type: task.task_type,
        priority: task.priority,
        status: task.status.as_str().to_string(),
        claimed_by: task.claimed_by,
        blocking_deps,
        unblocks,
        footprint_modifies,
        footprint_creates,
    })
}

/// Format task context as markdown for agent consumption
pub fn format_task_context(ctx: &TaskContext) -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!("# Task: {} - {}\n\n", ctx.task_id, ctx.title));

    // Task metadata
    out.push_str("## Overview\n\n");
    out.push_str(&format!("- **Type**: {}\n", ctx.task_type.label()));
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

/// Get task-type-specific guidance
fn get_type_specific_guidance(task_type: SqliteTaskType) -> String {
    match task_type {
        // Domain-based types (archetypes)
        SqliteTaskType::Frontend => r#"**Frontend Design Guidance:**
- Prioritize user experience and accessibility
- Follow design system patterns in the codebase
- Write semantic HTML with proper ARIA labels
- Test across viewports for responsive design
- Use existing component patterns
- Consider animation and transitions thoughtfully"#.to_string(),

        SqliteTaskType::Backend => r#"**Backend API Guidance:**
- Design APIs for extensibility and consistency
- Validate all inputs at system boundaries
- Handle errors gracefully with appropriate status codes
- Write efficient database queries
- Follow existing patterns in the codebase
- Consider authentication/authorization requirements"#.to_string(),

        SqliteTaskType::Data => r#"**Data Engineering Guidance:**
- Design for idempotency in pipelines
- Handle nulls and edge cases explicitly
- Optimize for query patterns
- Document data lineage and transformations
- Validate data at boundaries
- Consider schema migration impacts"#.to_string(),

        SqliteTaskType::Review => r#"**Code Review Guidance:**
- Review for correctness first, style second
- Provide specific, actionable feedback
- Check for edge cases and error paths
- Verify tests cover the changes
- Look for security implications
- Suggest improvements with examples"#.to_string(),

        SqliteTaskType::Security => r#"**Security Specialist Guidance:**
- Assume all input is malicious
- Check authentication on every endpoint
- Verify authorization checks exist
- Look for hardcoded secrets and credentials
- Review crypto implementations carefully
- Check for information disclosure risks
- Consider OWASP Top 10 vulnerabilities"#.to_string(),

        // Action-based types (legacy)
        SqliteTaskType::BugFix => r#"**Bug Fix Guidance:**
- First, reproduce the bug to understand it
- Identify the root cause before making changes
- Write a failing test that demonstrates the bug
- Fix the issue with minimal changes
- Verify the fix doesn't introduce regressions
- Document what caused the bug in commit message"#.to_string(),

        SqliteTaskType::Feature => r#"**Feature Implementation Guidance:**
- Understand the requirements fully before coding
- Consider edge cases and error handling
- Follow existing code patterns and conventions
- Add appropriate tests for the new functionality
- Update documentation if needed
- Keep commits focused and incremental"#.to_string(),

        SqliteTaskType::Refactor => r#"**Refactoring Guidance:**
- Ensure tests pass before starting
- Make small, incremental changes
- Each commit should leave tests passing
- Don't mix refactoring with feature changes
- Verify behavior is preserved after changes
- Document why the refactoring improves the code"#.to_string(),

        SqliteTaskType::Test => r#"**Test Writing Guidance:**
- Focus on testing behavior, not implementation
- Include both happy path and edge cases
- Use descriptive test names that explain intent
- Keep tests independent and isolated
- Avoid testing implementation details
- Aim for meaningful coverage, not 100%"#.to_string(),

        SqliteTaskType::Docs => r#"**Documentation Guidance:**
- Write for the intended audience
- Use clear, concise language
- Include examples where helpful
- Keep formatting consistent
- Verify all links and references work
- Update related docs if needed"#.to_string(),

        SqliteTaskType::Infra => r#"**Infrastructure Guidance:**
- Test changes in a safe environment first
- Document configuration changes
- Consider rollback procedures
- Update monitoring/alerting if needed
- Keep security implications in mind
- Verify CI/CD pipelines work correctly"#.to_string(),

        SqliteTaskType::Generic => r#"**General Guidance:**
- Understand the scope before starting
- Follow existing code conventions
- Test your changes thoroughly
- Keep commits focused and well-documented
- Ask for clarification if requirements are unclear"#.to_string(),
    }
}
