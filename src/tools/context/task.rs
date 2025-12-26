use crate::db;
use std::path::Path;
use rusqlite::OptionalExtension;

pub fn generate_task_context(bead_id: &str, _workspace_root: &Path) -> Result<String, String> {
    let claim_info = db::with_db(|conn| {
        conn.query_row(
            "SELECT agent_id, branch_name, claimed_at FROM claims WHERE bead_id = ?1",
            [bead_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            }
        ).optional()
    }).map_err(|e| e.to_string())?;

    let mut out = String::new();
    out.push_str(&format!("# Task Context: {}\n\n", bead_id));


    if let Some((agent, branch, _ts)) = claim_info {
        out.push_str(&format!("- **Status**: In Progress (Claimed by {})\n", agent));
        out.push_str(&format!("- **Branch**: `{}`\n", branch));
    } else {
        out.push_str("- **Status**: Unknown / Not Claimed\n");
    }

    out.push_str("\n## Objectives\n");
    out.push_str("1. Fulfill the requirements of this specific bead.\n");
    out.push_str("2. Ensure all tests pass within this isolated worktree.\n");
    out.push_str("3. Release the bead when done using `bacchus release`.\n");

    Ok(out)
}

