//! Validation logic for tasks: dependency cycles, footprint overlap, normalization.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::db::with_db;

use super::crud::list_sqlite_tasks;
use super::types::*;

// ============================================================================
// Footprint Helpers
// ============================================================================

/// Check if two footprints overlap
///
/// Handles wildcards: `file::*` overlaps with any `file::symbol`
#[cfg(test)]
pub fn footprints_overlap(a: &ResolvedFootprint, b: &ResolvedFootprint) -> bool {
    // Check exact symbol overlap
    if !a.symbols.is_disjoint(&b.symbols) {
        return true;
    }

    // Check wildcard overlap: file::* matches file::anything
    for sym_a in &a.symbols {
        for sym_b in &b.symbols {
            if symbols_match(sym_a, sym_b) {
                return true;
            }
        }
    }

    // Check creates overlap
    if !a.creates.is_disjoint(&b.creates) {
        return true;
    }

    // Check if any created file overlaps with modified symbols
    for create_path in &a.creates {
        for sym in &b.symbols {
            if symbols_match_file(create_path, sym) {
                return true;
            }
        }
    }
    for create_path in &b.creates {
        for sym in &a.symbols {
            if symbols_match_file(create_path, sym) {
                return true;
            }
        }
    }

    false
}

/// Check if two symbol patterns match (handles wildcards and bare file paths)
#[cfg(test)]
pub(crate) fn symbols_match(a: &str, b: &str) -> bool {
    // Exact match already handled by disjoint check
    if a == b {
        return true;
    }

    // Normalize bare file paths to file::* for comparison
    let norm_a = if a.contains("::") {
        a.to_string()
    } else {
        format!("{}::*", a)
    };
    let norm_b = if b.contains("::") {
        b.to_string()
    } else {
        format!("{}::*", b)
    };

    // Check if one is a wildcard for the other's file
    if let Some((file_a, sym_a)) = norm_a.rsplit_once("::") {
        if let Some((file_b, sym_b)) = norm_b.rsplit_once("::") {
            // Same file: wildcard matches any symbol
            if file_a == file_b && (sym_a == "*" || sym_b == "*") {
                return true;
            }
        }
    }

    false
}

/// Check if a file path matches a symbol pattern
#[cfg(test)]
pub(crate) fn symbols_match_file(file_path: &str, symbol: &str) -> bool {
    if let Some((file, _)) = symbol.rsplit_once("::") {
        file == file_path
    } else {
        // Symbol without :: is treated as file path
        symbol == file_path
    }
}

// ============================================================================
// Validation
// ============================================================================

/// Validate tasks using SQLite dependencies and footprints
pub fn validate_tasks(_workspace_root: &Path) -> Result<Vec<TaskValidation>, TasksError> {
    let tasks = list_sqlite_tasks(None, None, false)?;
    let mut validations: HashMap<String, TaskValidation> = tasks
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                TaskValidation {
                    task_id: t.id.clone(),
                    warnings: Vec::new(),
                    errors: Vec::new(),
                },
            )
        })
        .collect();

    // Validate footprint syntax based on normalized entries
    let footprint_rows = super::queries::get_footprint_rows(None).map_err(TasksError::DbError)?;

    for row in footprint_rows {
        let (task_id, pattern_type, file_path, symbol, is_wildcard) = (
            row.task_id,
            row.pattern_type,
            row.file_path,
            row.symbol,
            row.is_wildcard,
        );
        let Some(validation) = validations.get_mut(&task_id) else {
            continue;
        };

        if file_path.trim().is_empty() {
            validation
                .errors
                .push("Footprint has empty file path".to_string());
        }

        if is_wildcard == 0 && symbol.trim().is_empty() {
            validation
                .errors
                .push("Footprint symbol is empty without wildcard".to_string());
        }

        if is_wildcard == 1 && !symbol.is_empty() {
            validation
                .errors
                .push("Footprint wildcard has unexpected symbol".to_string());
        }

        if pattern_type != "modifies" && pattern_type != "creates" {
            validation.errors.push(format!(
                "Footprint has invalid pattern type: {}",
                pattern_type
            ));
        }
    }

    // Detect dependency cycles
    let deps_map =
        super::queries::get_all_deps().map_err(|e| TasksError::DbError(e.to_string()))?;

    let cycle_tasks = detect_dependency_cycles(&tasks, &deps_map);
    for task_id in cycle_tasks {
        if let Some(validation) = validations.get_mut(&task_id) {
            validation
                .errors
                .push("Dependency cycle detected".to_string());
        }
    }

    // Check footprint overlaps between open tasks
    let overlaps: Vec<(String, String, String)> = with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t1.id, t2.id, fp1.file_path
             FROM task_footprints fp1
             JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
               AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
             JOIN tasks t1 ON t1.id = fp1.task_id
             JOIN tasks t2 ON t2.id = fp2.task_id
             WHERE t1.id < t2.id
               AND t1.status = 'open'
               AND t2.status = 'open'
               AND t1.deleted_at IS NULL
               AND t2.deleted_at IS NULL",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .map_err(|e| TasksError::DbError(e.to_string()))?;

    for (task_a, task_b, file_path) in overlaps {
        if let Some(validation) = validations.get_mut(&task_a) {
            validation.warnings.push(format!(
                "Footprint overlaps with {} on {}",
                task_b, file_path
            ));
        }
        if let Some(validation) = validations.get_mut(&task_b) {
            validation.warnings.push(format!(
                "Footprint overlaps with {} on {}",
                task_a, file_path
            ));
        }
    }

    let mut ordered = Vec::new();
    for task in tasks {
        if let Some(validation) = validations.remove(&task.id) {
            ordered.push(validation);
        }
    }

    Ok(ordered)
}

pub(crate) fn detect_dependency_cycles(
    tasks: &[SqliteTask],
    deps: &HashMap<String, Vec<String>>,
) -> HashSet<String> {
    let mut visiting: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    let mut in_cycle: HashSet<String> = HashSet::new();

    fn dfs(
        node: &str,
        deps: &HashMap<String, Vec<String>>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
        stack: &mut Vec<String>,
        in_cycle: &mut HashSet<String>,
    ) {
        if visiting.contains(node) {
            if let Some(pos) = stack.iter().position(|n| n == node) {
                for id in &stack[pos..] {
                    in_cycle.insert(id.clone());
                }
            }
            return;
        }
        if visited.contains(node) {
            return;
        }

        visiting.insert(node.to_string());
        stack.push(node.to_string());

        if let Some(next) = deps.get(node) {
            for dep in next {
                dfs(dep, deps, visiting, visited, stack, in_cycle);
            }
        }

        stack.pop();
        visiting.remove(node);
        visited.insert(node.to_string());
    }

    for task in tasks {
        dfs(
            &task.id,
            deps,
            &mut visiting,
            &mut visited,
            &mut stack,
            &mut in_cycle,
        );
    }

    in_cycle
}

// ============================================================================
// Footprint Normalization
// ============================================================================

/// Normalize a TaskFootprint into NormalizedFootprint entries for SQLite storage
///
/// Uses split_once (first ::) to correctly handle nested symbols like file::Struct::method
/// which becomes file_path="file", symbol="Struct::method"
pub fn normalize_footprint(footprint: &TaskFootprint) -> Vec<NormalizedFootprint> {
    let mut normalized = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for pattern in &footprint.modifies {
        // Use split_once (first ::) to handle nested symbols like file::Foo::bar
        // This gives file_path="file", symbol="Foo::bar"
        if let Some((file_path, symbol_part)) = pattern.split_once("::") {
            if symbol_part == "*" || symbol_part.is_empty() {
                // Wildcard: file::* or malformed file:: -> (file, "", is_wildcard=1)
                let key = ("modifies".to_string(), file_path.to_string(), String::new());
                if seen.insert(key) {
                    normalized.push(NormalizedFootprint {
                        pattern_type: "modifies".to_string(),
                        file_path: file_path.to_string(),
                        symbol: String::new(),
                        is_wildcard: true,
                    });
                }
            } else {
                // Exact symbol: file::Symbol or file::Struct::method -> (file, Symbol/Struct::method, is_wildcard=0)
                let key = (
                    "modifies".to_string(),
                    file_path.to_string(),
                    symbol_part.to_string(),
                );
                if seen.insert(key) {
                    normalized.push(NormalizedFootprint {
                        pattern_type: "modifies".to_string(),
                        file_path: file_path.to_string(),
                        symbol: symbol_part.to_string(),
                        is_wildcard: false,
                    });
                }
            }
        } else {
            // Bare file path: file -> (file, "", is_wildcard=1)
            let key = ("modifies".to_string(), pattern.to_string(), String::new());
            if seen.insert(key) {
                normalized.push(NormalizedFootprint {
                    pattern_type: "modifies".to_string(),
                    file_path: pattern.to_string(),
                    symbol: String::new(),
                    is_wildcard: true,
                });
            }
        }
    }

    for path in &footprint.creates {
        // Creates are always wildcard (affects whole file)
        let key = ("creates".to_string(), path.to_string(), String::new());
        if seen.insert(key) {
            normalized.push(NormalizedFootprint {
                pattern_type: "creates".to_string(),
                file_path: path.to_string(),
                symbol: String::new(),
                is_wildcard: true,
            });
        }
    }

    normalized
}
