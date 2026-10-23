//! `ara fmt`: read (normalizing and repairing) and write back canonically.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use super::print_text;
use crate::project::Project;

/// Formats `files` in place, or with `check` only reports those that would change.
/// Files with errors are left untouched. `json` prints one report object instead.
pub fn fmt(project: &Project, files: &[PathBuf], check: bool, json: bool) -> Result<bool, String> {
    let mut ok = true;
    let mut changed = Vec::new();
    let mut skipped = Vec::new();
    for file in files {
        let rel = project.rel(file);
        let text = fs::read_to_string(file).map_err(|e| format!("cannot read `{rel}`: {e}"))?;
        let result = project.read_document(&text, &rel);
        let errors: Vec<_> = result
            .diags
            .iter()
            .filter(|d| d.is_error())
            .cloned()
            .collect();
        let Some(doc) = result.doc.filter(|_| errors.is_empty()) else {
            if !json {
                print_text(&errors);
                println!("skipped {rel}: fix the errors above first");
            }
            skipped.push(json!({ "file": rel, "diagnostics": errors }));
            ok = false;
            continue;
        };
        let formatted = project.write_document(&doc).map_err(|d| d.to_string())?;
        if formatted == text {
            continue;
        }
        if check {
            if !json {
                println!("would reformat {rel}");
            }
            ok = false;
        } else {
            fs::write(file, formatted).map_err(|e| format!("cannot write `{rel}`: {e}"))?;
            if !json {
                println!("formatted {rel}");
            }
        }
        changed.push(rel);
    }
    if json {
        let report =
            json!({ "files": files.len(), "check": check, "changed": changed, "skipped": skipped });
        print!("{}", ara_json::to_canonical_string(&report));
        return Ok(ok);
    }
    let verb = if check {
        "would reformat"
    } else {
        "reformatted"
    };
    println!("{}: {verb} {}", super::files(files.len()), changed.len());
    Ok(ok)
}
