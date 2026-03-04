//! Shared task query helpers used by multiple modules.
//!
//! Prevents duplication of common DB queries across context generation,
//! CLI commands, validation, and readiness checking.

use std::collections::HashMap;

use crate::db::with_db_str;

use super::types::TaskFootprint;

/// Get tasks that block a given task (unclosed dependencies).
pub fn get_blocking_deps(task_id: &str) -> Result<Vec<String>, String> {
    with_db_str(|conn| {
        let mut stmt = conn.prepare(
            "SELECT td.depends_on
             FROM task_dependencies td
             JOIN tasks dep ON dep.id = td.depends_on
             WHERE td.task_id = ?1
               AND dep.status != 'closed'
               AND dep.deleted_at IS NULL",
        )?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Get tasks that depend on a given task.
pub fn get_unblocks(task_id: &str) -> Result<Vec<String>, String> {
    with_db_str(|conn| {
        let mut stmt = conn.prepare(
            "SELECT td.task_id
             FROM task_dependencies td
             JOIN tasks t ON t.id = td.task_id
             WHERE td.depends_on = ?1
               AND t.deleted_at IS NULL",
        )?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Get all direct dependency IDs for a task (regardless of status).
pub fn get_depends_on(task_id: &str) -> Result<Vec<String>, String> {
    with_db_str(|conn| {
        let mut stmt =
            conn.prepare("SELECT depends_on FROM task_dependencies WHERE task_id = ?1")?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// A single row from the task_footprints table.
pub struct FootprintRow {
    pub task_id: String,
    pub pattern_type: String,
    pub file_path: String,
    pub symbol: String,
    pub is_wildcard: i32,
}

/// Fetch all footprint rows, optionally filtered to a single task.
pub fn get_footprint_rows(task_id: Option<&str>) -> Result<Vec<FootprintRow>, String> {
    with_db_str(|conn| {
        let sql = match task_id {
            Some(_) => "SELECT task_id, pattern_type, file_path, symbol, is_wildcard
                        FROM task_footprints WHERE task_id = ?1",
            None => "SELECT task_id, pattern_type, file_path, symbol, is_wildcard
                     FROM task_footprints",
        };
        let mut stmt = conn.prepare(sql)?;
        let map_row = |row: &rusqlite::Row| {
            Ok(FootprintRow {
                task_id: row.get(0)?,
                pattern_type: row.get(1)?,
                file_path: row.get(2)?,
                symbol: row.get(3)?,
                is_wildcard: row.get(4)?,
            })
        };
        let rows = match task_id {
            Some(id) => stmt.query_map([id], map_row)?,
            None => stmt.query_map([], map_row)?,
        };
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
}

/// Get footprint info for a task as modifies/creates lists.
pub fn get_task_footprint(task_id: &str) -> Result<TaskFootprint, String> {
    let rows = get_footprint_rows(Some(task_id))?;
    let mut footprint = TaskFootprint::default();

    for row in rows {
        match row.pattern_type.as_str() {
            "modifies" => {
                let pattern = if row.is_wildcard == 1 {
                    format!("{}::*", row.file_path)
                } else {
                    format!("{}::{}", row.file_path, row.symbol)
                };
                footprint.modifies.push(pattern);
            }
            "creates" => {
                footprint.creates.push(row.file_path);
            }
            _ => {}
        }
    }

    Ok(footprint)
}

/// Core SQL fragment for footprint-overlap join.
///
/// Shared between `get_footprint_conflicts` (standalone query) and
/// `readiness_predicates` (NOT EXISTS sub-select) to avoid divergence.
pub const FOOTPRINT_OVERLAP_JOIN: &str =
    "task_footprints fp1
     JOIN task_footprints fp2 ON fp1.file_path = fp2.file_path
       AND (fp1.is_wildcard = 1 OR fp2.is_wildcard = 1 OR fp1.symbol = fp2.symbol)
     JOIN tasks other ON other.id = fp2.task_id";

/// Get in-progress tasks with overlapping footprints.
pub fn get_footprint_conflicts(task_id: &str) -> Result<Vec<String>, String> {
    let sql = format!(
        "SELECT DISTINCT other.id
         FROM {FOOTPRINT_OVERLAP_JOIN}
         WHERE fp1.task_id = ?1
           AND other.id != ?1
           AND other.status = 'in_progress'
           AND other.deleted_at IS NULL"
    );
    with_db_str(|conn| {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([task_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Load all task dependency pairs as a map.
pub fn get_all_deps() -> Result<HashMap<String, Vec<String>>, String> {
    with_db_str(|conn| {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        let mut stmt = conn.prepare("SELECT task_id, depends_on FROM task_dependencies")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for (task_id, depends_on) in rows.flatten() {
            map.entry(task_id).or_default().push(depends_on);
        }
        Ok(map)
    })
}
