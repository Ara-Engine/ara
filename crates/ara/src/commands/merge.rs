//! `ara merge`: semantic three-way merge, usable as a git merge driver
//! (`driver = ara merge %O %A %B`).

use std::fs;
use std::path::{Path, PathBuf};

use ara_doc::{Document, read, write};
use serde_json::{Value, json};

use super::print_text;
use crate::project::Project;

/// Merges `[base, ours, theirs]` and writes the result to `output` (default: ours).
/// Returns `Ok(false)` when there are conflicts.
pub fn merge(
    project: &Project,
    files: [&PathBuf; 3],
    output: Option<&Path>,
    json: bool,
) -> Result<bool, String> {
    let mut docs: Vec<Document> = Vec::new();
    for file in files {
        let rel = project.rel(file);
        let text = fs::read_to_string(file).map_err(|e| format!("cannot read `{rel}`: {e}"))?;
        // References are left as written: temp files from git have no project path.
        let result = read(&text, &rel, &project.db, None);
        match result.doc {
            Some(doc) => docs.push(doc),
            None => {
                print_text(&result.diags);
                return Err(format!("cannot merge: `{rel}` is not a readable document"));
            }
        }
    }
    let [base, ours, theirs] = [&docs[0], &docs[1], &docs[2]];
    let merged = ara_merge::merge(base, ours, theirs, &project.db).map_err(|d| d.to_string())?;
    let target = output.unwrap_or(files[1].as_path());
    fs::write(target, write(&merged.doc, &project.db))
        .map_err(|e| format!("cannot write `{}`: {e}", target.display()))?;

    let shown = project.rel(target);
    if json {
        let conflicts: Vec<Value> = merged
            .conflicts
            .iter()
            .map(ara_merge::Conflict::to_json)
            .collect();
        print!(
            "{}",
            ara_json::to_canonical_string(&json!({ "output": shown, "conflicts": conflicts }))
        );
    } else {
        let diags: Vec<_> = merged
            .conflicts
            .iter()
            .map(|c| c.to_diagnostic(&shown))
            .collect();
        print_text(&diags);
        println!("merged into {shown}: {} conflicts", merged.conflicts.len());
    }
    Ok(merged.conflicts.is_empty())
}
