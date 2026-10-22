//! `ara check`: reader diagnostics (schema, references, structure) plus prefab
//! instance checks that need other documents.

use std::path::PathBuf;

use ara_diag::Diagnostic;
use ara_doc::AssetId;

use super::{json_report, print_text, tally};
use crate::project::Project;

/// Checks `files` (every project document if empty).
pub fn check(project: &Project, files: &[PathBuf], json: bool) -> Result<bool, String> {
    let targets: Vec<PathBuf> = if files.is_empty() {
        project.docs.clone()
    } else {
        files.to_vec()
    };
    let (ws, mut per_file) = project.load(&targets);
    let mut diags: Vec<Diagnostic> = project.diags.clone();
    for file in &targets {
        let rel = project.rel(file);
        diags.extend(per_file.remove(&rel).unwrap_or_default());
        let id = ws.find(&rel).cloned();
        if let Some(id) = id {
            diags.extend(instance_problems(project, &ws, &id));
        }
    }
    let (errors, warnings) = tally(&diags);
    if json {
        print!(
            "{}",
            ara_json::to_canonical_string(&json_report(targets.len(), &diags))
        );
    } else {
        print_text(&diags);
        println!(
            "checked {}: {errors} errors, {warnings} warnings",
            super::files(targets.len())
        );
    }
    Ok(errors == 0)
}

/// Problems only visible with the prefabs loaded: overrides, removals, nested parents.
fn instance_problems(project: &Project, ws: &ara_doc::Workspace, id: &AssetId) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    ws.resolve(id, &project.db, &mut out);
    out
}
