//! Symbol tools for querying

use crate::db::with_db;
use rusqlite::Result;
use serde::{Deserialize, Serialize};
use strsim::jaro_winkler;

const DEFAULT_LIMIT: i32 = 50;
const FUZZY_THRESHOLD: f64 = 0.7;

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
    pub search: Option<String>,
    pub fuzzy: bool,
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

// ============================================================================
// Tool Implementations
// ============================================================================

/// Find symbols matching given criteria
pub fn find_symbols(input: &FindSymbolsInput) -> Result<FindSymbolsOutput> {
    // Route to appropriate search method
    if let Some(ref query) = input.search {
        return search_symbols_fts(query, input.limit.unwrap_or(DEFAULT_LIMIT));
    }

    if input.fuzzy {
        if let Some(ref pattern) = input.pattern {
            return find_symbols_fuzzy(pattern, input.limit.unwrap_or(DEFAULT_LIMIT));
        }
    }

    // Default: SQL LIKE matching
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

/// Full-text search using FTS5
pub fn search_symbols_fts(query: &str, limit: i32) -> Result<FindSymbolsOutput> {
    with_db(|conn| {
        // FTS5 query with ranking using bm25
        let sql = r#"
            SELECT s.id, s.file, s.fq_name, s.kind, s.span_start_line, s.span_end_line,
                   s.line_count, s.hash, s.docstring, s.language
            FROM symbols_fts
            JOIN symbols s ON symbols_fts.rowid = s.id
            WHERE symbols_fts MATCH ?1
            ORDER BY bm25(symbols_fts)
            LIMIT ?2
        "#;

        let mut stmt = conn.prepare(sql)?;
        let symbols: Vec<SymbolInfo> = stmt
            .query_map(rusqlite::params![query, limit], |row| {
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

        // Get total count for matching results
        let count_sql = "SELECT COUNT(*) FROM symbols_fts WHERE symbols_fts MATCH ?1";
        let total_count: i32 = conn
            .query_row(count_sql, [query], |r| r.get(0))
            .unwrap_or(symbols.len() as i32);

        Ok(FindSymbolsOutput {
            symbols,
            total_count,
        })
    })
}

/// Fuzzy search using Jaro-Winkler similarity
pub fn find_symbols_fuzzy(query: &str, limit: i32) -> Result<FindSymbolsOutput> {
    with_db(|conn| {
        let query_lower = query.to_lowercase();

        // Get first character for prefix filtering (optimization)
        let first_char = query_lower.chars().next().unwrap_or('_');
        let prefix_pattern = format!("{}%", first_char);

        // Get candidate symbols with prefix filter
        let sql = r#"
            SELECT id, file, fq_name, kind, span_start_line, span_end_line,
                   line_count, hash, docstring, language
            FROM symbols
            WHERE LOWER(fq_name) LIKE ?1 OR LOWER(fq_name) LIKE ?2
        "#;

        // Also check if query appears anywhere (for middle matches)
        let contains_pattern = format!("%{}%", query_lower);

        let mut stmt = conn.prepare(sql)?;
        let mut candidates: Vec<(SymbolInfo, f64)> = stmt
            .query_map(rusqlite::params![prefix_pattern, contains_pattern], |row| {
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
            .filter_map(|sym| {
                // Score each symbol using Jaro-Winkler on the name part
                let name = sym.fq_name.split("::").last().unwrap_or(&sym.fq_name);
                let score = jaro_winkler(&name.to_lowercase(), &query_lower);
                if score >= FUZZY_THRESHOLD {
                    Some((sym, score))
                } else {
                    None
                }
            })
            .collect();

        // Sort by score descending
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let total_count = candidates.len() as i32;
        let symbols: Vec<SymbolInfo> = candidates
            .into_iter()
            .take(limit as usize)
            .map(|(s, _)| s)
            .collect();

        Ok(FindSymbolsOutput {
            symbols,
            total_count,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{close_db, init_db};
    use tempfile::tempdir;

    fn setup_test_db() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        init_db(Some(db_path.to_str().unwrap()), true).unwrap();
        dir
    }

    #[test]
    fn test_find_symbols_empty() {
        let _dir = setup_test_db();

        let input = FindSymbolsInput {
            pattern: None,
            kind: None,
            file: None,
            language: None,
            limit: Some(10),
            search: None,
            fuzzy: false,
        };

        let result = find_symbols(&input).unwrap();
        assert_eq!(result.total_count, 0);
        assert!(result.symbols.is_empty());

        close_db();
    }
}
