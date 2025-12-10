//! Symbol tools for querying and context retrieval

use crate::db::with_db;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Default token budget for context
const DEFAULT_TOKEN_BUDGET: i32 = 8000;
const CHARS_PER_TOKEN: f32 = 3.5;
const DEFAULT_LIMIT: i32 = 50;

// ============================================================================
// Input/Output Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct FindSymbolsInput {
    pub pattern: Option<String>,
    pub kind: Option<String>,
    pub file: Option<String>,
    pub language: Option<String>,
    pub limit: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FindSymbolsOutput {
    pub symbols: Vec<SymbolInfo>,
    pub total_count: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub id: i64,
    pub file: String,
    pub fq_name: String,
    pub kind: String,
    pub span_start_line: i32,
    pub span_end_line: i32,
    pub line_count: i32,
    pub hash: String,
    pub docstring: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetTaskContextInput {
    pub bead_id: String,
    pub token_budget: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetTaskContextOutput {
    pub code_snippets: Vec<CodeSnippet>,
    pub related_beads: Vec<RelatedBead>,
    pub doc_fragments: Vec<DocFragment>,
    pub warnings: Vec<String>,
    pub estimated_tokens: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CodeSnippet {
    pub fq_name: String,
    pub file: String,
    pub kind: String,
    pub content: String,
    pub start_line: i32,
    pub end_line: i32,
    pub relevance: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RelatedBead {
    pub bead_id: String,
    pub title: Option<String>,
    pub relation: String,
    pub symbols_in_common: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DocFragment {
    pub id: String,
    pub path: Option<String>,
    pub anchor: Option<String>,
    pub content_preview: String,
}

// ============================================================================
// Tool Implementations
// ============================================================================

/// Find symbols matching given criteria
pub fn find_symbols(input: &FindSymbolsInput) -> Result<FindSymbolsOutput> {
    with_db(|conn| {
        let mut conditions = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref pattern) = input.pattern {
            conditions.push("fq_name LIKE ?");
            params_vec.push(Box::new(pattern.replace('*', "%")));
        }

        if let Some(ref kind) = input.kind {
            conditions.push("kind = ?");
            params_vec.push(Box::new(kind.clone()));
        }

        if let Some(ref file) = input.file {
            conditions.push("file LIKE ?");
            params_vec.push(Box::new(file.replace('*', "%")));
        }

        if let Some(ref language) = input.language {
            conditions.push("language = ?");
            params_vec.push(Box::new(language.clone()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let limit = input.limit.unwrap_or(DEFAULT_LIMIT);

        // Get total count
        let count_sql = format!("SELECT COUNT(*) FROM symbols {}", where_clause);
        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        let total_count: i32 = conn.query_row(&count_sql, params_refs.as_slice(), |row| row.get(0))?;

        // Get symbols
        let query_sql = format!(
            "SELECT id, file, fq_name, kind, span_start_line, span_end_line, line_count, hash, docstring, language FROM symbols {} ORDER BY file, span_start_line LIMIT ?",
            where_clause
        );

        let mut all_params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
        all_params.push(&limit);

        let mut stmt = conn.prepare(&query_sql)?;
        let symbols: Vec<SymbolInfo> = stmt
            .query_map(rusqlite::params_from_iter(all_params), |row| {
                Ok(SymbolInfo {
                    id: row.get(0)?,
                    file: row.get(1)?,
                    fq_name: row.get(2)?,
                    kind: row.get(3)?,
                    span_start_line: row.get(4)?,
                    span_end_line: row.get(5)?,
                    line_count: row.get(6)?,
                    hash: row.get(7)?,
                    docstring: row.get(8)?,
                    language: row.get(9)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(FindSymbolsOutput {
            symbols,
            total_count,
        })
    })
}

/// Get context bundle for a task
pub fn get_task_context(input: &GetTaskContextInput, workspace_root: &Path) -> Result<GetTaskContextOutput> {
    let token_budget = input.token_budget.unwrap_or(DEFAULT_TOKEN_BUDGET);
    let char_budget = (token_budget as f32 * CHARS_PER_TOKEN) as i32;

    let mut warnings = Vec::new();
    let mut code_snippets = Vec::new();
    let mut related_beads = Vec::new();
    let mut doc_fragments = Vec::new();
    let mut used_chars = 0;

    with_db(|conn| {
        // Check if task exists
        let task_exists: bool = conn
            .query_row(
                "SELECT 1 FROM tasks WHERE bead_id = ?1",
                [&input.bead_id],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !task_exists {
            return Ok(GetTaskContextOutput {
                code_snippets: vec![],
                related_beads: vec![],
                doc_fragments: vec![],
                warnings: vec!["Task not found in coordination tracking".to_string()],
                estimated_tokens: 0,
            });
        }

        // Step 1: Get directly related symbols
        let mut stmt = conn.prepare(
            "SELECT s.id, s.file, s.fq_name, s.kind, s.span_start_line, s.span_end_line, bs.relation FROM bead_symbols bs JOIN symbols s ON bs.symbol_id = s.id WHERE bs.bead_id = ?1 ORDER BY bs.relation DESC"
        )?;

        let direct_symbols: Vec<(i64, String, String, String, i32, i32, String)> = stmt
            .query_map([&input.bead_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let direct_files: HashSet<String> = direct_symbols.iter().map(|(_, f, _, _, _, _, _)| f.clone()).collect();
        let direct_ids: HashSet<i64> = direct_symbols.iter().map(|(id, _, _, _, _, _, _)| *id).collect();
        let direct_fq_names: HashSet<String> = direct_symbols.iter().map(|(_, _, fq, _, _, _, _)| fq.clone()).collect();

        // Add direct symbols to snippets
        for (_, file, fq_name, kind, start_line, end_line, _) in &direct_symbols {
            if used_chars >= char_budget {
                break;
            }

            if let Some(content) = read_symbol_content(workspace_root, file, *start_line, *end_line) {
                if used_chars + content.len() as i32 <= char_budget {
                    code_snippets.push(CodeSnippet {
                        fq_name: fq_name.clone(),
                        file: file.to_string(),
                        kind: kind.clone(),
                        content: content.clone(),
                        start_line: *start_line,
                        end_line: *end_line,
                        relevance: "direct".to_string(),
                    });
                    used_chars += content.len() as i32;
                }
            }
        }

        // Step 2: Get neighbor symbols (same file)
        for file in &direct_files {
            if used_chars >= char_budget {
                break;
            }

            let mut stmt = conn.prepare(
                "SELECT id, fq_name, kind, span_start_line, span_end_line FROM symbols WHERE file = ?1 ORDER BY span_start_line"
            )?;

            let neighbors: Vec<(i64, String, String, i32, i32)> = stmt
                .query_map([file], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                })?
                .filter_map(|r| r.ok())
                .collect();

            for (id, fq_name, kind, start_line, end_line) in neighbors {
                if direct_ids.contains(&id) {
                    continue;
                }
                if used_chars >= char_budget {
                    break;
                }

                if let Some(content) = read_symbol_content(workspace_root, file, start_line, end_line) {
                    // Reserve 30% for other context
                    if used_chars + content.len() as i32 <= (char_budget as f32 * 0.7) as i32 {
                        code_snippets.push(CodeSnippet {
                            fq_name,
                            file: file.to_string(),
                            kind,
                            content: content.clone(),
                            start_line,
                            end_line,
                            relevance: "neighbor".to_string(),
                        });
                        used_chars += content.len() as i32;
                    }
                }
            }
        }

        // Step 3: Find related beads
        if !direct_fq_names.is_empty() {
            let placeholders: Vec<String> = (0..direct_fq_names.len()).map(|i| format!("?{}", i + 1)).collect();
            let sql = format!(
                "SELECT bs.bead_id, t.title, bs.symbol_ref FROM bead_symbols bs LEFT JOIN tasks t ON bs.bead_id = t.bead_id WHERE bs.symbol_ref IN ({}) AND bs.bead_id != ?{}",
                placeholders.join(", "),
                direct_fq_names.len() + 1
            );

            let mut stmt = conn.prepare(&sql)?;
            let mut params: Vec<String> = direct_fq_names.iter().cloned().collect();
            params.push(input.bead_id.clone());

            let overlapping: Vec<(String, Option<String>, String)> = stmt
                .query_map(rusqlite::params_from_iter(&params), |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .filter_map(|r| r.ok())
                .collect();

            // Group by bead
            let mut bead_map: std::collections::HashMap<String, (Option<String>, Vec<String>)> =
                std::collections::HashMap::new();
            for (bead_id, title, symbol_ref) in overlapping {
                bead_map
                    .entry(bead_id)
                    .or_insert_with(|| (title, Vec::new()))
                    .1
                    .push(symbol_ref);
            }

            for (bead_id, (title, symbols)) in bead_map {
                related_beads.push(RelatedBead {
                    bead_id,
                    title,
                    relation: "overlapping".to_string(),
                    symbols_in_common: symbols,
                });
            }
        }

        // Step 4: Get doc fragments (simplified)
        if !direct_fq_names.is_empty() {
            let placeholders: Vec<String> = (0..direct_fq_names.len()).map(|i| format!("?{}", i + 1)).collect();
            let sql = format!(
                "SELECT id, path, anchor, content_markdown FROM doc_fragments WHERE scope_ref IN ({}) LIMIT 5",
                placeholders.join(", ")
            );

            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<String> = direct_fq_names.iter().cloned().collect();

            let fragments: Vec<(String, Option<String>, Option<String>, Option<String>)> = stmt
                .query_map(rusqlite::params_from_iter(&params), |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .filter_map(|r| r.ok())
                .collect();

            for (id, path, anchor, content) in fragments {
                if used_chars >= char_budget {
                    break;
                }

                let preview = content.map(|c| {
                    let truncated: String = c.chars().take(200).collect();
                    if c.len() > 200 {
                        format!("{}...", truncated)
                    } else {
                        truncated
                    }
                }).unwrap_or_default();

                if !preview.is_empty() && used_chars + preview.len() as i32 <= char_budget {
                    doc_fragments.push(DocFragment {
                        id,
                        path,
                        anchor,
                        content_preview: preview.clone(),
                    });
                    used_chars += preview.len() as i32;
                }
            }
        }

        Ok(GetTaskContextOutput {
            code_snippets,
            related_beads,
            doc_fragments,
            warnings,
            estimated_tokens: (used_chars as f32 / CHARS_PER_TOKEN) as i32,
        })
    })
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Read the content of a symbol from its file
fn read_symbol_content(workspace_root: &Path, file: &str, start_line: i32, end_line: i32) -> Option<String> {
    let file_path = workspace_root.join(file);
    let content = fs::read_to_string(&file_path).ok()?;
    let lines: Vec<&str> = content.lines().collect();

    let start_idx = (start_line - 1) as usize;
    let end_idx = end_line as usize;

    if start_idx < lines.len() && end_idx <= lines.len() {
        Some(lines[start_idx..end_idx].join("\n"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use tempfile::tempdir;

    fn setup_test_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap()), true).unwrap();
    }

    #[test]
    fn test_find_symbols_empty() {
        setup_test_db();

        let input = FindSymbolsInput {
            pattern: None,
            kind: None,
            file: None,
            language: None,
            limit: Some(10),
        };

        let result = find_symbols(&input).unwrap();
        assert_eq!(result.total_count, 0);
        assert!(result.symbols.is_empty());
    }
}
