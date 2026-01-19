use crate::db;
use std::path::Path;
use rusqlite::OptionalExtension;

pub fn generate_task_context(task_id: &str, _workspace_root: &Path) -> Result<String, String> {
    let claim_info = db::with_db(|conn| {
        conn.query_row(
            "SELECT claimed_by, claimed_at FROM tasks_v2 WHERE id = ?1 AND deleted_at IS NULL",
            [task_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            }
        ).optional()
    }).map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str(&format!("# Task Context: {}\n\n", task_id));

    if let Some((Some(agent), _ts)) = claim_info {
        out.push_str(&format!("- **Status**: In Progress (Claimed by {})\n", agent));
        out.push_str(&format!("- **Branch**: `bacchus/{}`\n", task_id));
    } else if claim_info.is_some() {
        out.push_str("- **Status**: Not Claimed\n");
    } else {
        out.push_str("- **Status**: Unknown (Task not found)\n");
    }

    out.push_str("\n## Objectives\n");
    out.push_str("1. Fulfill the requirements of this task.\n");
    out.push_str("2. Ensure all tests pass within this isolated worktree.\n");
    out.push_str("3. Release when done using `bacchus release`.\n");

    Ok(out)
}

