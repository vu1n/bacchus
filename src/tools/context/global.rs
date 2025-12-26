use crate::db;
use crate::beads;
use std::path::Path;

pub fn generate_global_context(_workspace_root: &Path) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("# Project Orchestration Context\n\n");
    out.push_str("You are the **Orchestrator Agent**. Your goal is to coordinate work using `beads` and `bacchus`.\n\n");

    // 1. Active Claims (Who is doing what?)
    out.push_str("## Active Claims\n");
    let claims = db::with_db(|conn| {
         let mut stmt = conn.prepare("SELECT bead_id, agent_id FROM claims")?;
         let rows = stmt.query_map([], |row| {
             Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
         })?;
         rows.collect::<Result<Vec<_>, _>>()
    }).map_err(|e| e.to_string())?;

    if claims.is_empty() {
        out.push_str("_No active claims._\n");
    } else {        out.push_str("| Bead ID | Agent |\n|---|---|\n");
        for (bead, agent) in claims {
            out.push_str(&format!("| {} | {} |\n", bead, agent));
        }
    }

    // 2. Ready Work (What can be assigned?)
    out.push_str("\n## Ready for Assignment\n");
    let ready_beads = beads::get_ready_beads().map_err(|e| e.to_string())?;
    
    if ready_beads.is_empty() {
        out.push_str("_No ready beads. Use `beads-planner` to create new work._\n");
    } else {
        out.push_str("| Bead ID | Title |\n|---|---|\n");
        for bead in ready_beads.iter().take(10) {
            let id = &bead.id;
            let title = &bead.title;
            out.push_str(&format!("| {} | {} |\n", id, title));
        }
        if ready_beads.len() > 10 {
            out.push_str(&format!("_...and {} more._\n", ready_beads.len() - 10));
        }
    }

    Ok(out)
}

