use crate::db;
use crate::tasks;
use std::path::Path;

pub fn generate_global_context(_workspace_root: &Path) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("# Project Orchestration Context\n\n");
    out.push_str(
        "You are the **Orchestrator Agent**. Your goal is to coordinate work using `bacchus`.\n\n",
    );

    // 0. Ready Work (avoid nested DB calls)
    let ready_tasks = tasks::get_ready_sqlite_tasks(None).map_err(|e| e.to_string())?;

    // 1. Active Claims (Who is doing what?)
    out.push_str("## Active Claims\n");
    let claims = db::with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, claimed_by FROM tasks
              WHERE status = 'in_progress' AND claimed_by IS NOT NULL AND deleted_at IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
    .map_err(|e| e.to_string())?;

    if claims.is_empty() {
        out.push_str("_No active claims._\n");
    } else {
        out.push_str("| Task ID | Agent |\n|---|---|\n");
        for (task, agent) in claims {
            out.push_str(&format!("| {} | {} |\n", task, agent));
        }
    }

    // 2. Ready Work (What can be assigned?)
    out.push_str("\n## Ready for Assignment\n");

    if ready_tasks.is_empty() {
        out.push_str(
            "_No ready tasks. Add tasks to `.bacchus/tasks.yaml` and run `bacchus task import`._\n",
        );
    } else {
        out.push_str("| Task ID | Title |\n|---|---|\n");
        for task in ready_tasks.iter().take(10) {
            let id = &task.id;
            let title = &task.title;
            out.push_str(&format!("| {} | {} |\n", id, title));
        }
        if ready_tasks.len() > 10 {
            out.push_str(&format!("_...and {} more._\n", ready_tasks.len() - 10));
        }
    }

    Ok(out)
}
