//! Bacchus - Worktree-based coordination CLI for multi-agent work

mod beads;
mod cli;
mod config;
mod db;
mod indexer;
mod tools;
mod updater;
mod worktree;

use clap::Parser;
use cli::{Cli, Commands};
use std::path::PathBuf;

fn main() {
    let cli = Cli::parse();

    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Initialize database (check BACCHUS_DB_PATH env var first)
    let db_path = std::env::var("BACCHUS_DB_PATH").ok();
    let db_path_str = db_path.as_deref().unwrap_or_else(|| {
        // Use default path relative to workspace
        ".bacchus/bacchus.db"
    });

    if let Err(e) = db::init_db(Some(db_path_str), true) {
        eprintln!("Failed to initialize database: {}", e);
        std::process::exit(1);
    }

    let result = match cli.command {
        // ====================================================================
        // Coordination Commands
        // ====================================================================
        Commands::Next { agent_id } => {
            tools::next_task(&agent_id, &workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Release { bead_id, status } => {
            tools::release_bead(&bead_id, &status, &workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(e.to_string()),
                ))
        }

        Commands::Abort { bead_id } => {
            tools::abort_merge(&bead_id, &workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(e.to_string()),
                ))
        }

        Commands::Resolve { bead_id } => {
            tools::resolve_merge(&bead_id, &workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(e.to_string()),
                ))
        }

        Commands::Stale { minutes, cleanup } => {
            tools::find_stale(minutes, cleanup, &workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(e.to_string()),
                ))
        }

        Commands::List => {
            tools::list_claims().map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        // ====================================================================
        // Symbol Commands
        // ====================================================================
        Commands::Symbols { pattern, kind, file, lang, limit, search, fuzzy } => {
            let input = tools::FindSymbolsInput {
                pattern,
                kind,
                file,
                language: lang,
                limit: Some(limit),
                search,
                fuzzy,
            };
            tools::find_symbols(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
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
        // Info Commands
        // ====================================================================
        Commands::Status => {
            get_status().map(|r| serde_json::to_string_pretty(&r).unwrap())
        }

        Commands::Workflow => {
            println!("{}", WORKFLOW_DOC);
            Ok(String::new())
        }

        // ====================================================================
        // Update Commands
        // ====================================================================
        Commands::SelfUpdate => {
            updater::self_update().map(|v| {
                serde_json::json!({
                    "success": true,
                    "updated_to": v
                }).to_string()
            }).map_err(|e| rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(e.to_string()),
            ))
        }

        Commands::CheckUpdate => {
            updater::check_for_updates().map(|info| {
                serde_json::to_string_pretty(&info).unwrap()
            }).map_err(|e| rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(e.to_string()),
            ))
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
    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    db::with_db(|conn| {
        // Count claims
        let claims_count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM claims",
            [],
            |r| r.get(0),
        ).unwrap_or(0);

        // Get active claims with worktree paths
        let mut stmt = conn.prepare(
            "SELECT bead_id, agent_id, worktree_path, branch_name,
                    (strftime('%s', 'now') * 1000 - claimed_at) / 60000 as age_minutes
             FROM claims"
        )?;
        let claims: Vec<(serde_json::Value, String)> = stmt
            .query_map([], |row| {
                let worktree_path: String = row.get(2)?;
                Ok((serde_json::json!({
                    "bead_id": row.get::<_, String>(0)?,
                    "agent_id": row.get::<_, String>(1)?,
                    "worktree_path": &worktree_path,
                    "branch": row.get::<_, String>(3)?,
                    "age_minutes": row.get::<_, i64>(4)?
                }), worktree_path))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let claim_values: Vec<serde_json::Value> = claims.iter().map(|(v, _)| v.clone()).collect();
        let claimed_worktrees: std::collections::HashSet<String> =
            claims.iter().map(|(_, p)| p.clone()).collect();

        // Count symbols indexed
        let symbols_count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM symbols",
            [],
            |r| r.get(0),
        ).unwrap_or(0);

        // Get ready beads count from beads
        let ready_count = beads::get_ready_beads()
            .map(|v| v.len())
            .unwrap_or(0);

        // Check for orphaned worktrees (worktrees on disk without claims)
        let worktrees_dir = std::env::var("BACCHUS_WORKTREES")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workspace_root.join(".bacchus/worktrees"));

        let mut orphaned_worktrees: Vec<String> = Vec::new();
        if worktrees_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&worktrees_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_dir() {
                        let path_str = path.to_string_lossy().to_string();
                        if !claimed_worktrees.contains(&path_str) {
                            orphaned_worktrees.push(
                                path.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path_str)
                            );
                        }
                    }
                }
            }
        }

        // Check for broken claims (claims where worktree doesn't exist)
        let broken_claims: Vec<String> = claims.iter()
            .filter(|(_, path)| !PathBuf::from(path).exists())
            .filter_map(|(v, _)| v.get("bead_id").and_then(|b| b.as_str()).map(String::from))
            .collect();

        Ok(serde_json::json!({
            "claims": {
                "count": claims_count,
                "active": claim_values
            },
            "symbols_indexed": symbols_count,
            "ready_beads": ready_count,
            "orphaned_worktrees": orphaned_worktrees,
            "broken_claims": broken_claims
        }))
    })
}

const WORKFLOW_DOC: &str = r#"
# Bacchus Coordination Protocol

## Agent Workflow

1. **Get Work**
   ```bash
   bacchus next <agent_id>
   ```
   - Finds ready bead from beads DB (open, no blockers)
   - Creates worktree at .bacchus/worktrees/{bead_id}/
   - Claims bead, updates status to in_progress

2. **Do Work**
   Work in the worktree. All changes are isolated on branch bacchus/{bead_id}.

3. **Release When Done**
   ```bash
   # Success - merge to main and cleanup
   bacchus release <bead_id> --status done

   # Blocked - keep worktree, release claim
   bacchus release <bead_id> --status blocked

   # Failed - discard worktree, reset bead
   bacchus release <bead_id> --status failed
   ```

4. **Handle Merge Conflicts**
   If release fails due to conflicts:
   ```bash
   # Option 1: Resolve manually then complete
   # ... fix conflicts, git add resolved files ...
   bacchus resolve <bead_id>

   # Option 2: Abort and keep working
   bacchus abort <bead_id>
   ```

## Stale Detection

Find abandoned claims:
```bash
bacchus stale --minutes 30

# Auto-cleanup stale claims
bacchus stale --minutes 30 --cleanup
```

## Code Search

```bash
bacchus index src/
bacchus symbols --pattern "User*" --kind class
```

## Status

```bash
bacchus status
```
"#;
