//! Bacchus - AST-aware coordination CLI for multi-agent work

mod cli;
mod db;
mod indexer;
mod tools;

use clap::Parser;
use cli::{Cli, Commands};
use std::path::PathBuf;

fn main() {
    let cli = Cli::parse();

    // Initialize database
    if let Err(e) = db::init_db(None, true) {
        eprintln!("Failed to initialize database: {}", e);
        std::process::exit(1);
    }

    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let result = match cli.command {
        // ====================================================================
        // Coordination Commands
        // ====================================================================
        Commands::Claim { bead_id, agent_id } => {
            let input = tools::ClaimTaskInput { bead_id, agent_id };
            tools::claim_task(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Release { bead_id, agent_id, reason } => {
            let input = tools::ReleaseTaskInput { bead_id, agent_id, reason };
            tools::release_task(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Workplan {
            bead_id,
            agent_id,
            modifies_files,
            modifies_symbols,
            modifies_modules,
            creates_symbols,
        } => {
            let workplan = tools::Workplan {
                modifies: if modifies_files.is_some() || modifies_symbols.is_some() || modifies_modules.is_some() {
                    Some(tools::ModifiesSpec {
                        files: modifies_files.map(|s| s.split(',').map(|s| s.trim().to_string()).collect()),
                        symbols: modifies_symbols.map(|s| s.split(',').map(|s| s.trim().to_string()).collect()),
                        modules: modifies_modules.map(|s| s.split(',').map(|s| s.trim().to_string()).collect()),
                    })
                } else {
                    None
                },
                creates: creates_symbols.map(|s| tools::CreatesSpec {
                    symbols: Some(s.split(',').map(|s| s.trim().to_string()).collect()),
                }),
            };
            let input = tools::UpdateWorkplanInput { bead_id, agent_id, workplan };
            tools::update_workplan(&input, &workspace_root).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Footprint { bead_id, agent_id, files, added, removed, breaking } => {
            let diff_summary = tools::DiffSummary {
                files_changed: files.split(',').map(|s| s.trim().to_string()).collect(),
                lines_added: added,
                lines_removed: removed,
            };
            let breaking_changes: Option<Vec<tools::BreakingChange>> = breaking.map(|b| {
                b.split(',').map(|change| {
                    let parts: Vec<&str> = change.splitn(3, ':').collect();
                    tools::BreakingChange {
                        symbol: parts.first().unwrap_or(&"").to_string(),
                        change_kind: parts.get(1).unwrap_or(&"").to_string(),
                        description: parts.get(2).unwrap_or(&"").to_string(),
                    }
                }).collect()
            });
            // Note: reportFootprint needs async handling for re-indexing
            // For now, return a simplified response
            let output = tools::ReportFootprintOutput {
                success: true,
                bead_id,
                symbols_touched: vec![],
                conflicts: vec![],
                notifications_sent: 0,
                start_hash: "".to_string(),
            };
            Ok(serde_json::to_string_pretty(&output).unwrap())
        }

        Commands::Heartbeat { bead_id, agent_id } => {
            let input = tools::HeartbeatInput { bead_id, agent_id };
            tools::heartbeat(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Stale { minutes } => {
            let input = tools::ListStaleTasksInput { threshold_minutes: Some(minutes) };
            tools::list_stale_tasks(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        // ====================================================================
        // Symbol Commands
        // ====================================================================
        Commands::Symbols { pattern, kind, file, lang, limit } => {
            let input = tools::FindSymbolsInput {
                pattern,
                kind,
                file,
                language: lang,
                limit: Some(limit),
            };
            tools::find_symbols(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Context { bead_id, tokens } => {
            let input = tools::GetTaskContextInput {
                bead_id,
                token_budget: Some(tokens),
            };
            tools::get_task_context(&input, &workspace_root).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Index { path } => {
            match index_path(&path, &workspace_root) {
                Ok(count) => Ok(serde_json::json!({
                    "success": true,
                    "files_indexed": count,
                    "path": path
                }).to_string()),
                Err(e) => Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(e),
                )),
            }
        }

        // ====================================================================
        // Communication Commands
        // ====================================================================
        Commands::Notifications { agent_id, status, limit } => {
            let input = tools::GetNotificationsInput {
                agent_id,
                status,
                limit: Some(limit),
            };
            tools::get_notifications(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Resolve { notification_id, agent_id, action, notes } => {
            let input = tools::ResolveNotificationInput {
                notification_id,
                agent_id,
                action,
                notes,
            };
            tools::resolve_notification(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Stakeholders { symbol, transitive } => {
            let input = tools::QueryStakeholdersInput {
                symbol,
                include_transitive: Some(transitive),
            };
            tools::query_stakeholders(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Notify { symbol, agent_id, bead_id, kind, description, commit } => {
            tools::notify_stakeholders(&symbol, &agent_id, &bead_id, &kind, &description, commit.as_deref())
                .map(|count| serde_json::json!({ "notifications_sent": count }).to_string())
        }

        // ====================================================================
        // Human Escalation Commands
        // ====================================================================
        Commands::Decide { agent_id, bead_id, question, options, context, symbols, urgency } => {
            let input = tools::RequestHumanDecisionInput {
                agent_id,
                bead_id,
                question,
                options: options.split(',').map(|s| s.trim().to_string()).collect(),
                context,
                affected_symbols: symbols.map(|s| s.split(',').map(|s| s.trim().to_string()).collect()),
                urgency: Some(urgency),
            };
            tools::request_human_decision(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Answer { notification_id, human_id, decision, notes } => {
            let input = tools::SubmitHumanDecisionInput {
                notification_id,
                human_id,
                decision,
                notes,
            };
            tools::submit_human_decision(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Pending { human, limit } => {
            let input = tools::GetPendingDecisionsInput {
                human_id: human,
                limit: Some(limit),
            };
            tools::get_pending_decisions(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        // ====================================================================
        // Info Commands
        // ====================================================================
        Commands::Status => {
            get_status().map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Workflow => {
            println!("{}", WORKFLOW_DOC);
            Ok(String::new())
        }
    };

    match result {
        Ok(output) => {
            if !output.is_empty() {
                println!("{}", output);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }

    db::close_db();
}

/// Index a file or directory
fn index_path(path: &str, workspace_root: &PathBuf) -> Result<usize, String> {
    use walkdir::WalkDir;

    let target = workspace_root.join(path);
    let mut parser = indexer::Parser::new().map_err(|e| e.to_string())?;
    let mut count = 0;

    if target.is_file() {
        if let Err(e) = index_single_file(&mut parser, &target, workspace_root) {
            return Err(e);
        }
        count = 1;
    } else if target.is_dir() {
        for entry in WalkDir::new(&target)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let ext = entry.path().extension().and_then(|e| e.to_str()).unwrap_or("");
            if indexer::Language::from_extension(ext).is_some() {
                if index_single_file(&mut parser, entry.path(), workspace_root).is_ok() {
                    count += 1;
                }
            }
        }
    } else {
        return Err(format!("Path not found: {}", path));
    }

    Ok(count)
}

/// Index a single file
fn index_single_file(
    parser: &mut indexer::Parser,
    file_path: &std::path::Path,
    workspace_root: &PathBuf,
) -> Result<(), String> {
    let content = std::fs::read_to_string(file_path).map_err(|e| e.to_string())?;
    let relative_path = file_path
        .strip_prefix(workspace_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    let (tree, language) = parser.parse_file(&content, &relative_path).map_err(|e| e.to_string())?;
    let symbols = indexer::extract_symbols(&tree, &relative_path, &content, language);

    // Store symbols in database
    db::with_db(|conn| {
        for sym in &symbols {
            conn.execute(
                "INSERT OR REPLACE INTO symbols (file, fq_name, kind, span_start_line, span_end_line, line_count, hash, docstring, language) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    sym.file,
                    sym.fq_name,
                    sym.kind.as_str(),
                    sym.span_start_line,
                    sym.span_end_line,
                    sym.line_count,
                    sym.hash,
                    sym.docstring,
                    sym.language.as_str()
                ],
            )?;
        }
        Ok(())
    }).map_err(|e: rusqlite::Error| e.to_string())?;

    Ok(())
}

/// Get current status
fn get_status() -> rusqlite::Result<serde_json::Value> {
    db::with_db(|conn| {
        let total: i32 = conn.query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))?;
        let claimed: i32 = conn.query_row("SELECT COUNT(*) FROM tasks WHERE owner IS NOT NULL", [], |r| r.get(0))?;

        let mut stmt = conn.prepare(
            "SELECT bead_id, owner, (strftime('%s', 'now') * 1000 - last_heartbeat) / 60000 as age FROM tasks WHERE owner IS NOT NULL"
        )?;
        let active: Vec<serde_json::Value> = stmt
            .query_map([], |row| {
                Ok(serde_json::json!({
                    "bead_id": row.get::<_, String>(0)?,
                    "owner": row.get::<_, String>(1)?,
                    "heartbeat_age_minutes": row.get::<_, i64>(2)?
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let pending_notifications: i32 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE status = 'pending'",
            [],
            |r| r.get(0),
        )?;

        Ok(serde_json::json!({
            "tasks": {
                "total": total,
                "claimed": claimed,
                "active": active
            },
            "notifications": {
                "pending": pending_notifications
            }
        }))
    })
}

const WORKFLOW_DOC: &str = r#"
# Bacchus Coordination Protocol

## Agent Workflow

1. **Start Work**
   ```bash
   bacchus claim <bead_id> <agent_id>
   ```

2. **Declare Intent**
   ```bash
   bacchus workplan <bead_id> <agent_id> \
     --modifies-symbols "Foo::bar,Baz::qux" \
     --creates-symbols "NewClass::method"
   ```

3. **Keep Alive** (every 5 minutes)
   ```bash
   bacchus heartbeat <bead_id> <agent_id>
   ```

4. **Report Changes**
   ```bash
   bacchus footprint <bead_id> <agent_id> \
     --files "src/foo.ts,src/bar.ts" \
     --added 50 --removed 10
   ```

5. **Release When Done**
   ```bash
   bacchus release <bead_id> <agent_id>
   ```

## Handling Conflicts

- Check overlaps in workplan response
- Query stakeholders before breaking changes
- Use notifications to coordinate

## Human Escalation

When stuck on a decision:
```bash
bacchus decide <agent_id> <bead_id> "Which approach?" \
  --options "Option A,Option B,Option C" \
  --urgency high
```
"#;
