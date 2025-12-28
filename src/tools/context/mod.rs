use std::path::Path;

use crate::worktree;

mod global;
mod task;

/// Generate context for the current agent
pub fn generate_context(bead_id_opt: Option<String>, workspace_root: &Path) -> Result<String, String> {
    let current_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    
    // Check if we are inside the .bacchus/worktrees directory of the workspace
    let worktrees_dir = worktree::get_worktrees_dir(workspace_root);
    let is_worktree = current_dir.starts_with(&worktrees_dir);
    
    // Determine the target bead_id
    let target_bead_id = if let Some(id) = bead_id_opt {
        Some(id)
    } else if is_worktree {
        // We are in .../.bacchus/worktrees/<BEAD_ID>/...
        // We want the component right after 'worktrees'
        current_dir.strip_prefix(&worktrees_dir)
            .ok()
            .and_then(|p| p.iter().next())
            .map(|s| s.to_string_lossy().to_string())
    } else {
        None
    };

    if let Some(bead_id) = target_bead_id {
        task::generate_task_context(&bead_id, workspace_root)
    } else {
        global::generate_global_context(workspace_root)
    }
}
