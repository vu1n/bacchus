//! Bacchus - Workspace-based coordination CLI for multi-agent work

mod cli;
mod config;
mod db;
mod epics;
mod events;
mod handles;
mod indexer;
mod messages;
mod tasks;
mod tools;
mod updater;
mod workers;
mod workspace;

use clap::Parser;
use cli::{
    ArchetypeCommands, Cli, Commands, EpicCommands, HandleCommands, MessageCommands,
    SessionCommands, TaskCommands,
};
use std::path::PathBuf;

const SESSION_SCOPE_ENV_KEYS: [&str; 3] = [
    "BACCHUS_SESSION_ID",
    "CLAUDE_SESSION_ID",
    "CLAUDE_CONVERSATION_ID",
];

fn main() {
    let cli = Cli::parse();

    // Determine workspace root by traversing up
    let workspace_root = find_workspace_root()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // Fast path for `session check` when no session exists
    // This avoids creating .bacchus/ in repos that don't use bacchus
    if let Commands::Session {
        command: SessionCommands::Check,
    } = &cli.command
    {
        let has_session = has_session_file_for_scope(&workspace_root);
        if !has_session {
            // No session file = no bacchus session active, approve immediately
            println!(r#"{{"decision":"approve","reason":"No bacchus session active"}}"#);
            return;
        }
    }

    // Initialize database (check BACCHUS_DB_PATH env var first)
    let db_path = std::env::var("BACCHUS_DB_PATH").ok();
    let db_path_buf = if let Some(p) = db_path {
        PathBuf::from(p)
    } else {
        workspace_root.join(".bacchus/bacchus.db")
    };

    let db_path_str = db_path_buf.to_str().unwrap_or(".bacchus/bacchus.db");

    if let Err(e) = db::init_db(Some(db_path_str)) {
        eprintln!("Failed to initialize database: {}", e);
        std::process::exit(1);
    }

    let result = match cli.command {
        // ====================================================================
        // Coordination Commands
        // ====================================================================
        Commands::Next { agent_id } => tools::next_task(&agent_id, &workspace_root)
            .map(|r| serde_json::to_string_pretty(&r).unwrap()),

        Commands::Claim {
            task_id,
            agent_id,
            force,
        } => tools::claim_task(&task_id, &agent_id, force, &workspace_root)
            .map(|r| serde_json::to_string_pretty(&r).unwrap()),

        Commands::Release { task_id, status } => {
            tools::release_task(&task_id, &status, &workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(e.to_string()),
                    )
                })
        }

        Commands::Abort { task_id } => tools::abort_merge(&task_id, &workspace_root)
            .map(|r| serde_json::to_string_pretty(&r).unwrap())
            .map_err(|e| {
                rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
            }),

        Commands::Resolve { task_id } => tools::resolve_merge(&task_id, &workspace_root)
            .map(|r| serde_json::to_string_pretty(&r).unwrap())
            .map_err(|e| {
                rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
            }),

        Commands::Stale { minutes, cleanup } => {
            tools::find_stale(minutes, cleanup, &workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(e.to_string()),
                    )
                })
        }

        Commands::List => tools::list_claims().map(|r| serde_json::to_string_pretty(&r).unwrap()),

        Commands::Heartbeat { task_id, agent_id } => {
            tasks::heartbeat_sqlite_task(&task_id, &agent_id)
                .map(|_| {
                    serde_json::json!({
                        "success": true,
                        "task_id": task_id,
                        "agent_id": agent_id,
                        "message": "Heartbeat recorded"
                    })
                    .to_string()
                })
                .map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(e.to_string()),
                    )
                })
        }

        Commands::Review {
            task_id,
            build_cmd,
            test_cmd,
        } => tools::review_task(
            &task_id,
            &workspace_root,
            build_cmd.as_deref(),
            test_cmd.as_deref(),
        )
        .map(|r| serde_json::to_string_pretty(&r).unwrap())
        .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),

        Commands::ProcessReleases { limit } => {
            let run_id = format!(
                "manual-orchestrator-{}-{}",
                chrono::Utc::now().timestamp_millis(),
                std::process::id()
            );
            match tasks::try_acquire_orchestrator_lease(&run_id, tasks::ORCHESTRATOR_LEASE_TTL_MS) {
                Ok(true) => {
                    let result =
                        tools::process_ready_releases(&workspace_root, Some(limit), Some(&run_id))
                            .map(|r| serde_json::to_string_pretty(&r).unwrap())
                            .map_err(|e| {
                                rusqlite::Error::SqliteFailure(
                                    rusqlite::ffi::Error::new(1),
                                    Some(e),
                                )
                            });
                    let _ = tasks::release_orchestrator_lease(&run_id);
                    result
                }
                Ok(false) => {
                    let lease_msg = tasks::get_orchestrator_lease()
                        .ok()
                        .flatten()
                        .map(|l| {
                            format!("holder={} expires_at={}", l.holder_id, l.lease_expires_at)
                        })
                        .unwrap_or_else(|| "holder=unknown".to_string());
                    Err(rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(format!(
                            "Another orchestrator leader lease is active ({}).",
                            lease_msg
                        )),
                    ))
                }
                Err(e) => Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(e.to_string()),
                )),
            }
        }

        Commands::Eval { epic, days } => tools::generate_eval_report(epic.as_deref(), days)
            .map(|r| serde_json::to_string_pretty(&r).unwrap())
            .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),

        Commands::Init {
            skip_jj,
            force_tasks,
            epic_id,
            epic_title,
        } => tools::init_workspace(
            &workspace_root,
            tools::InitOptions {
                skip_jj,
                force_tasks,
                epic_id: epic_id.as_deref(),
                epic_title: epic_title.as_deref(),
            },
        )
        .map(|r| serde_json::to_string_pretty(&r).unwrap())
        .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),

        // ====================================================================
        // Symbol Commands
        // ====================================================================
        Commands::Symbols {
            pattern,
            kind,
            file,
            lang,
            limit,
            search,
            fuzzy,
            handle,
        } => {
            let input = tools::FindSymbolsInput {
                pattern,
                kind,
                file,
                language: lang,
                limit: Some(limit),
                search,
                fuzzy,
                handle,
            };
            if handle {
                tools::find_symbols_handle(&input)
                    .map(|r| serde_json::to_string_pretty(&r).unwrap())
            } else {
                tools::find_symbols(&input).map(|r| serde_json::to_string_pretty(&r).unwrap())
            }
        }

        Commands::Index { path } => match index_path(&path, &workspace_root) {
            Ok(count) => Ok(serde_json::json!({
                "success": true,
                "files_indexed": count,
                "path": path
            })
            .to_string()),
            Err(e) => Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(e),
            )),
        },

        // ====================================================================
        // Info Commands
        // ====================================================================
        Commands::Status => get_status().map(|r| serde_json::to_string_pretty(&r).unwrap()),

        Commands::Workflow => {
            println!("{}", WORKFLOW_DOC);
            Ok(String::new())
        }

        // ====================================================================
        // Update Commands
        // ====================================================================
        Commands::SelfUpdate => updater::self_update()
            .map(|v| {
                serde_json::json!({
                    "success": true,
                    "updated_to": v
                })
                .to_string()
            })
            .map_err(|e| {
                rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
            }),

        Commands::Context { task_id } => tools::generate_context(task_id, &workspace_root)
            .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),

        Commands::CheckUpdate => updater::check_for_updates()
            .map(|info| serde_json::to_string_pretty(&info).unwrap())
            .map_err(|e| {
                rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
            }),

        Commands::Events { limit } => events::list_recent_events(limit)
            .map(|v| serde_json::to_string_pretty(&v).unwrap())
            .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),

        // ====================================================================
        // Session Commands (for stop hooks)
        // ====================================================================
        Commands::Session { command } => match command {
            SessionCommands::Start {
                mode,
                task_id,
                max_concurrent,
                agent_id,
            } => tools::start_session(
                &mode,
                task_id.as_deref(),
                max_concurrent,
                agent_id.as_deref(),
            )
            .map(|msg| serde_json::json!({"success": true, "message": msg}).to_string())
            .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            SessionCommands::Stop => tools::stop_session()
                .map(|msg| serde_json::json!({"success": true, "message": msg}).to_string())
                .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            SessionCommands::Status => tools::session_status()
                .map(|v| serde_json::to_string_pretty(&v).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            SessionCommands::Check => {
                let result = tools::check_session();
                Ok(serde_json::to_string_pretty(&result).unwrap())
            }
            SessionCommands::SpawnWorkers { count, dry_run } => {
                tools::spawn_workers_once(count, dry_run)
                    .map(|v| serde_json::to_string_pretty(&v).unwrap())
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))
                    })
            }
            SessionCommands::Prune { minutes } => tools::prune_sessions(minutes)
                .map(|v| serde_json::to_string_pretty(&v).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            SessionCommands::HeartbeatLoop {
                task_id,
                agent_id,
                token,
                interval_ms,
            } => tools::run_agent_heartbeat_loop(&task_id, &agent_id, &token, interval_ms)
                .map(|msg| serde_json::json!({"success": true, "message": msg}).to_string())
                .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            SessionCommands::LeaseLoop {
                run_id,
                token,
                interval_ms,
            } => tools::run_orchestrator_lease_loop(&run_id, &token, interval_ms)
                .map(|msg| serde_json::json!({"success": true, "message": msg}).to_string())
                .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            SessionCommands::WorkerRun {
                worker_id,
                run_id,
                task_id,
                agent_id,
                scope_id,
                command,
            } => tools::run_worker_command(
                worker_id, &run_id, &task_id, &agent_id, &scope_id, &command,
            )
            .map(|msg| serde_json::json!({"success": true, "message": msg}).to_string())
            .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
        },

        // ====================================================================
        // Task Commands (built-in task management)
        // ====================================================================
        Commands::Task { command } => match command {
            TaskCommands::List { status, ready } => {
                tools::list_tasks(&workspace_root, status.as_deref(), ready)
                    .map(|r| serde_json::to_string_pretty(&r).unwrap())
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))
                    })
            }
            TaskCommands::Show { id } => tools::show_task(&workspace_root, &id)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            TaskCommands::Validate => tools::validate_tasks(&workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            TaskCommands::Init => tools::init_tasks(&workspace_root)
                .map(|r| serde_json::to_string_pretty(&r).unwrap())
                .map_err(|e| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))),
            TaskCommands::Import { epic_id } => {
                tools::import_tasks(&workspace_root, epic_id.as_deref())
                    .map(|r| serde_json::to_string_pretty(&r).unwrap())
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e))
                    })
            }
        },

        // ====================================================================
        // Epic Commands (hierarchical orchestration)
        // ====================================================================
        Commands::Epic { command } => match command {
            EpicCommands::List { status } => {
                let status_filter = status
                    .as_ref()
                    .and_then(|s| epics::EpicStatus::from_str(s).ok());
                epics::list_epics(status_filter)
                    .map(|epics| serde_json::to_string_pretty(&epics).unwrap())
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(1),
                            Some(e.to_string()),
                        )
                    })
            }
            EpicCommands::Show { id } => epics::get_epic_with_counts(&id)
                .map(|epic| serde_json::to_string_pretty(&epic).unwrap())
                .map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(e.to_string()),
                    )
                }),
            EpicCommands::Create {
                id,
                title,
                description,
            } => {
                let input = epics::CreateEpicInput {
                    id,
                    title,
                    description,
                    created_by: "human".to_string(),
                };
                epics::create_epic(input)
                    .map(|epic| serde_json::to_string_pretty(&epic).unwrap())
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(1),
                            Some(e.to_string()),
                        )
                    })
            }
            EpicCommands::Assign { id, agent } => epics::assign_epic(&id, &agent)
                .map(|epic| {
                    serde_json::json!({
                        "success": true,
                        "epic": epic,
                        "message": format!("Epic {} assigned to {}", id, agent)
                    })
                    .to_string()
                })
                .map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(e.to_string()),
                    )
                }),
            EpicCommands::SetStatus { id, status } => match epics::EpicStatus::from_str(&status) {
                Ok(parsed_status) => epics::update_epic_status(&id, parsed_status)
                    .map(|epic| serde_json::to_string_pretty(&epic).unwrap())
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(1),
                            Some(e.to_string()),
                        )
                    }),
                Err(e) => Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(e.to_string()),
                )),
            },
        },

        // ====================================================================
        // Message Commands (agent communication)
        // ====================================================================
        Commands::Message { command } => match command {
            MessageCommands::List { agent, status } => {
                let status_filter = status
                    .as_ref()
                    .and_then(|s| messages::MessageStatus::from_str(s).ok());
                messages::list_messages(agent.as_deref(), status_filter)
                    .map(|msgs| serde_json::to_string_pretty(&msgs).unwrap())
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(1),
                            Some(e.to_string()),
                        )
                    })
            }
            MessageCommands::Send {
                agent,
                message_type,
                payload,
            } => match serde_json::from_str::<serde_json::Value>(&payload) {
                Ok(payload_json) => {
                    let input = messages::SendMessageInput {
                        target_agent: agent,
                        message_type,
                        payload: payload_json,
                    };
                    messages::send_message(input)
                        .map(|msg| serde_json::to_string_pretty(&msg).unwrap())
                        .map_err(|e| {
                            rusqlite::Error::SqliteFailure(
                                rusqlite::ffi::Error::new(1),
                                Some(e.to_string()),
                            )
                        })
                }
                Err(e) => Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(format!("Invalid JSON payload: {}", e)),
                )),
            },
            MessageCommands::Claim { agent, limit } => messages::claim_messages(&agent, limit)
                .map(|msgs| serde_json::to_string_pretty(&msgs).unwrap())
                .map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(e.to_string()),
                    )
                }),
            MessageCommands::Ack { message_id, agent } => {
                messages::mark_processed(message_id, &agent)
                    .map(|_| {
                        serde_json::json!({
                            "success": true,
                            "message_id": message_id,
                            "agent": agent,
                            "status": "processed"
                        })
                        .to_string()
                    })
                    .map_err(|e| {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(1),
                            Some(e.to_string()),
                        )
                    })
            }
            MessageCommands::Fail {
                message_id,
                agent,
                reason,
            } => messages::mark_failed(message_id, &agent, reason.as_deref())
                .map(|_| {
                    serde_json::json!({
                        "success": true,
                        "message_id": message_id,
                        "agent": agent,
                        "status": "failed"
                    })
                    .to_string()
                })
                .map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(e.to_string()),
                    )
                }),
            MessageCommands::ReclaimStale => messages::reclaim_stale_messages()
                .map(|(requeued, failed)| {
                    serde_json::json!({
                        "success": true,
                        "requeued": requeued,
                        "failed": failed
                    })
                    .to_string()
                })
                .map_err(|e| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(1),
                        Some(e.to_string()),
                    )
                }),
        },

        // ====================================================================
        // Archetype Commands
        // ====================================================================
        Commands::Archetype { command } => match command {
            ArchetypeCommands::List => Ok(tools::cmd_list_archetypes()),
            ArchetypeCommands::Show { name } => Ok(tools::cmd_show_archetype(&name)),
            ArchetypeCommands::Prompt { name } => Ok(tools::cmd_archetype_prompt(&name)),
            ArchetypeCommands::Select { task_id } => Ok(tools::cmd_select_archetype(&task_id)),
        },

        // ====================================================================
        // Handle Commands (token-saving query results)
        // ====================================================================
        Commands::Handle { command } => match command {
            HandleCommands::Expand {
                handle,
                limit,
                offset,
            } => handles::expand_handle(&handle, Some(limit), Some(offset))
                .map(|data| serde_json::to_string_pretty(&data).unwrap()),
            HandleCommands::Filter { handle, kind, file } => handles::filter_handle(
                &handle,
                |v| {
                    let kind_match = kind
                        .as_ref()
                        .is_none_or(|k| v["kind"].as_str() == Some(k.as_str()));
                    let file_match = file.as_ref().is_none_or(|f| {
                        v["file"].as_str().is_some_and(|vf| {
                            if f.contains('*') {
                                let pattern = f.replace('*', "");
                                vf.contains(&pattern)
                            } else {
                                vf.contains(f)
                            }
                        })
                    });
                    kind_match && file_match
                },
                |v| v["fq_name"].as_str().unwrap_or("?").to_string(),
            )
            .map(|stub| serde_json::to_string_pretty(&stub).unwrap()),
            HandleCommands::List => handles::list_handles()
                .map(|handles| serde_json::to_string_pretty(&handles).unwrap()),
            HandleCommands::Clear => handles::clear_all_handles().map(|count| {
                serde_json::json!({
                    "success": true,
                    "cleared": count
                })
                .to_string()
            }),
            HandleCommands::Info { handle } => handles::get_handle_info(&handle)
                .map(|info| serde_json::to_string_pretty(&info).unwrap()),
        },
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

/// Index a file or directory (parallelized with rayon)
fn index_path(path: &str, workspace_root: &PathBuf) -> Result<usize, String> {
    use rayon::prelude::*;
    use walkdir::WalkDir;

    let target = workspace_root.join(path);

    if target.is_file() {
        // Single file - no parallelization needed
        let mut parser = indexer::Parser::new().map_err(|e| e.to_string())?;
        let symbols = parse_file(&mut parser, &target, workspace_root)?;
        store_symbols(&symbols)?;
        return Ok(1);
    }

    if !target.is_dir() {
        return Err(format!("Path not found: {}", path));
    }

    // Collect all indexable files first
    let files: Vec<PathBuf> = WalkDir::new(&target)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let ext = e.path().extension().and_then(|e| e.to_str()).unwrap_or("");
            indexer::Language::from_extension(ext).is_some()
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    // Parse files in parallel (each thread gets its own parser)
    let all_symbols: Vec<indexer::ExtractedSymbol> = files
        .par_iter()
        .filter_map(|file_path| {
            // Create parser per thread (tree-sitter parsers aren't thread-safe)
            let mut parser = indexer::Parser::new().ok()?;
            parse_file(&mut parser, file_path, workspace_root).ok()
        })
        .flatten()
        .collect();

    let file_count = files.len();

    // Batch insert all symbols (single DB transaction)
    store_symbols(&all_symbols)?;

    Ok(file_count)
}

/// Find workspace root by looking for .bacchus or .git directories walking up
///
/// Priority:
/// 1. CLAUDE_PROJECT_DIR env var (set by Claude Code for plugins/hooks)
/// 2. Walk up from CWD looking for .bacchus or .git
fn find_workspace_root() -> Option<PathBuf> {
    // First check CLAUDE_PROJECT_DIR (set by Claude Code for hooks/plugins)
    if let Ok(project_dir) = std::env::var("CLAUDE_PROJECT_DIR") {
        let path = PathBuf::from(&project_dir);
        if path.exists() {
            return Some(path);
        }
    }

    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join(".bacchus").exists() {
            return Some(current);
        }

        // If we hit .git, we are likely at root, UNLESS it's a worktree .git file
        let git_path = current.join(".git");
        if git_path.exists() && git_path.is_dir() {
            return Some(current);
        }
        if git_path.exists() {
            // If .git is a file, it's a submodule or worktree.
            // If worktree, we should keep going up to find the real root.
            // But we might be in a submodule which IS a root for its own context?
            // For bacchus, we care about where .bacchus is.
        }

        if !current.pop() {
            break;
        }
    }
    None
}

fn sanitize_scope(scope: &str) -> String {
    let mut out = String::with_capacity(scope.len());
    for ch in scope.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

fn current_session_scope_id() -> String {
    for key in SESSION_SCOPE_ENV_KEYS {
        if let Ok(v) = std::env::var(key) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return sanitize_scope(trimmed);
            }
        }
    }
    "default".to_string()
}

fn has_session_file_for_scope(workspace_root: &std::path::Path) -> bool {
    let scope_id = current_session_scope_id();
    let scoped = workspace_root
        .join(".bacchus")
        .join("sessions")
        .join(format!("{}.json", scope_id));
    let legacy = workspace_root.join(".bacchus/session.json");
    scoped.exists() || legacy.exists()
}

/// Parse a single file and extract symbols
fn parse_file(
    parser: &mut indexer::Parser,
    file_path: &std::path::Path,
    workspace_root: &PathBuf,
) -> Result<Vec<indexer::ExtractedSymbol>, String> {
    let content = std::fs::read_to_string(file_path).map_err(|e| e.to_string())?;
    let relative_path = file_path
        .strip_prefix(workspace_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string();

    let (tree, language) = parser
        .parse_file(&content, &relative_path)
        .map_err(|e| e.to_string())?;
    Ok(indexer::extract_symbols(
        &tree,
        &relative_path,
        &content,
        language,
    ))
}

/// Store symbols in database (batched in single transaction)
/// Clears existing symbols for each file before inserting to prevent stale accumulation
fn store_symbols(symbols: &[indexer::ExtractedSymbol]) -> Result<(), String> {
    db::with_db(|conn| {
        // Collect unique files being indexed
        let files: std::collections::HashSet<_> = symbols.iter().map(|s| s.file.as_str()).collect();

        // Delete existing symbols for these files (handles deleted/renamed symbols)
        for file in &files {
            // Delete from FTS first (triggers won't fire on direct FTS delete)
            conn.execute(
                "DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file = ?1)",
                [file],
            )?;
            conn.execute("DELETE FROM symbols WHERE file = ?1", [file])?;
        }

        // Insert new symbols using OR REPLACE to handle duplicates
        // (e.g., TS function overloads with same fq_name - last definition wins)
        for sym in symbols {
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
    }).map_err(|e: rusqlite::Error| e.to_string())
}

/// Get current status
fn get_status() -> rusqlite::Result<serde_json::Value> {
    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Get ready tasks count BEFORE with_db to avoid deadlock
    let ready_count = tasks::get_ready_sqlite_tasks(None)
        .map(|v| v.len())
        .unwrap_or(0);

    db::with_db(|conn| {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let active_cutoff = now_ms - tasks::CLAIM_HEARTBEAT_TIMEOUT_MS;

        // Get in_progress claims and split into heartbeat-fresh active vs stale.
        let mut stmt = conn.prepare(
            "SELECT id, claimed_by, claimed_at, claimed_heartbeat_at,
                    CASE WHEN COALESCE(claimed_heartbeat_at, claimed_at, 0) >= ?1 THEN 1 ELSE 0 END AS is_active
             FROM tasks
             WHERE status = 'in_progress' AND claimed_by IS NOT NULL AND deleted_at IS NULL",
        )?;
        let claims: Vec<(serde_json::Value, String, bool)> = stmt
            .query_map([active_cutoff], |row| {
                let task_id: String = row.get(0)?;
                let claimed_at: Option<i64> = row.get(2)?;
                let heartbeat_at: Option<i64> = row.get(3)?;
                let is_active = row.get::<_, i32>(4)? == 1;
                let last_seen = heartbeat_at.or(claimed_at).unwrap_or(0);
                let age_minutes = if last_seen > 0 {
                    (now_ms - last_seen) / 60000
                } else {
                    0
                };
                let workspace_path = format!(".bacchus/workspaces/{}", task_id);
                Ok((
                    serde_json::json!({
                        "task_id": &task_id,
                        "agent_id": row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        "workspace_path": &workspace_path,
                        "age_minutes": age_minutes,
                        "last_seen_at": last_seen
                    }),
                    workspace_path,
                    is_active,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let claim_values: Vec<serde_json::Value> = claims
            .iter()
            .filter(|(_, _, is_active)| *is_active)
            .map(|(v, _, _)| v.clone())
            .collect();
        let stale_claim_values: Vec<serde_json::Value> = claims
            .iter()
            .filter(|(_, _, is_active)| !*is_active)
            .map(|(v, _, _)| v.clone())
            .collect();
        let claims_count = claim_values.len() as i32;
        let claimed_task_ids: std::collections::HashSet<String> = claims
            .iter()
            .filter_map(|(v, _, _)| v.get("task_id").and_then(|t| t.as_str()).map(String::from))
            .collect();

        // Count symbols indexed
        let symbols_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap_or(0);

        // Check for orphaned workspaces (workspaces on disk without claims)
        let workspaces_dir = std::env::var("BACCHUS_WORKSPACES")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workspace_root.join(".bacchus/workspaces"));

        let mut orphaned_workspaces: Vec<String> = Vec::new();
        if workspaces_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&workspaces_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                            if !claimed_task_ids.contains(dir_name) {
                                orphaned_workspaces.push(dir_name.to_string());
                            }
                        }
                    }
                }
            }
        }

        // Check for broken claims (claims where workspace doesn't exist)
        let broken_claims: Vec<String> = claims
            .iter()
            .filter(|(_, path, _)| !workspace_root.join(path).exists())
            .filter_map(|(v, _, _)| v.get("task_id").and_then(|b| b.as_str()).map(String::from))
            .collect();

        Ok(serde_json::json!({
            "claims": {
                "count": claims_count,
                "active": claim_values,
                "stale_count": stale_claim_values.len(),
                "stale": stale_claim_values
            },
            "symbols_indexed": symbols_count,
            "ready_tasks": ready_count,
            "orphaned_workspaces": orphaned_workspaces,
            "broken_claims": broken_claims
        }))
    })
}

const WORKFLOW_DOC: &str = r#"
# Bacchus Coordination Protocol

## Swarm Methodology

Control loop:
1. Plan tasks with dependencies/footprints
2. Admit only ready non-overlapping tasks
3. Execute in isolated jj workspaces
4. Reconcile releases and stale/failed workers
5. Measure with eval metrics

## Initialization

Run once per repo:

```bash
if ! jj root >/dev/null 2>&1; then
  jj git init --colocate
  jj config set --repo user.name "Your Name"
  jj config set --repo user.email "you@example.com"
  jj bookmark create main -r @
  jj describe -m "Initialize jj for bacchus"
  jj new
fi
```

## Task Management

Tasks are defined in `.bacchus/tasks.yaml`:

```bash
# One-shot bootstrap (jj + .bacchus/tasks.yaml, optional epic)
bacchus init

# Initialize tasks.yaml template
bacchus task init

# List all tasks
bacchus task list

# List ready tasks only
bacchus task list --ready

# Show task details
bacchus task show <task_id>

# Validate tasks against symbol index
bacchus task validate
```

## Agent Workflow

1. **Get Work**
   ```bash
   bacchus next <agent_id>
   ```
   - Finds ready task (open, dependencies satisfied, no footprint conflicts)
   - Creates jj workspace at .bacchus/workspaces/{task_id}/
   - Claims task, updates status to in_progress

2. **Do Work**
   Work in the workspace. Changes are auto-snapshotted by jj.

3. **Release When Done**
   ```bash
   # Success - mark ready for release (orchestrator will merge)
   bacchus release <task_id> --status done

   # Orchestrator merge step (manual trigger outside session hooks)
   bacchus process-releases

   # Blocked - keep workspace, release claim
   bacchus release <task_id> --status blocked

   # Failed - discard workspace, reset task
   bacchus release <task_id> --status failed
   ```

4. **Handle Merge Conflicts**
   If release fails due to conflicts:
   ```bash
   # Option 1: Resolve manually then complete
   # ... fix conflicts, git add resolved files ...
   bacchus resolve <task_id>

   # Option 2: Abort and keep working
   bacchus abort <task_id>
   ```

## Orchestrator Session Workflow

```bash
# Start orchestrator lease/session
bacchus session start orchestrator --max-concurrent 3

# Optional: configure autonomous worker launch on `session check`
export BACCHUS_WORKER_CMD='claude'
export BACCHUS_ORCHESTRATOR_AUTO_SPAWN=1
export BACCHUS_WORKER_STALE_GRACE_MS=60000
# Optional runtime safety budget (disabled when unset)
export BACCHUS_WORKER_MAX_RUNTIME_MS=1800000
# Optional best-effort stale PID termination
export BACCHUS_WORKER_KILL_STALE=1

# Manually launch ready workers once
bacchus session spawn-workers --count 3

# Preview launch candidates and slot pressure without spawning
bacchus session spawn-workers --count 3 --dry-run

# Stop when done
bacchus session stop
```

## Collision Detection

Tasks can define footprints to prevent parallel agents from modifying the same code:

```yaml
tasks:
  - id: AUTH-001
    title: Implement authentication
    footprint:
      modifies:
        - "src/auth/handler.rs::AuthHandler"
        - "src/auth/jwt.rs::*"  # All symbols in file
      creates:
        - "src/auth/middleware.rs"
```

Tasks with overlapping footprints won't both be marked as ready.

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

## Context

```bash
bacchus context
bacchus context --task-id <task_id>
```

- Run from repo root for global context.
- Run inside a workspace for task context.
- Task context includes active footprint collisions and risk hints.

## Status

```bash
bacchus status
```

## Review

```bash
# Advisory checks before release (workspace, changes, footprint, optional build/test)
bacchus review <task_id> --build-cmd "cargo build" --test-cmd "cargo test -q"
```

Footprint review is symbol-aware: changed hunks must fall within declared symbol spans.

## Prompt Starter

Paste into your LLM:

```text
Use bacchus to implement: <GOAL>
1) Plan/update .bacchus/tasks.yaml with dependencies, footprints, archetypes
2) Import tasks and verify ready queue
3) Start orchestrator session (max-concurrent 3)
4) Dry-run spawn, then launch workers
5) Process releases and recover stale/failed workers until done
6) Finish with bacchus eval --days 7 and risk summary
```
"#;
