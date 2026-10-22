//! `ara fmt`: read (normalizing and repairing) and write back canonically.

use std::fs;
use std::path::PathBuf;

use ara_doc::{read, write};

use super::print_text;
use crate::project::Project;

/// Formats `files` in place, or with `check` only reports those that would change.
/// Files with errors are left untouched.
pub fn fmt(project: &Project, files: &[PathBuf], check: bool) -> Result<bool, String> {
    let mut ok = true;
    let mut changed = 0;
    for file in files {
        let rel = project.rel(file);
        let text = fs::read_to_string(file).map_err(|e| format!("cannot read `{rel}`: {e}"))?;
        let result = read(&text, &rel, &project.db, Some(&project.index));
        let errors: Vec<_> = result
            .diags
            .iter()
            .filter(|d| d.is_error())
            .cloned()
            .collect();
        let Some(doc) = result.doc.filter(|_| errors.is_empty()) else {
            print_text(&errors);
            println!("skipped {rel}: fix the errors above first");
            ok = false;
            continue;
        };
        let formatted = write(&doc, &project.db);
        if formatted == text {
            continue;
        }
        changed += 1;
        if check {
            println!("would reformat {rel}");
            ok = false;
        } else {
            fs::write(file, formatted).map_err(|e| format!("cannot write `{rel}`: {e}"))?;
            println!("formatted {rel}");
        }
    }
    let verb = if check {
        "would reformat"
    } else {
        "reformatted"
    };
    println!("{}: {verb} {changed}", super::files(files.len()));
    Ok(ok)
}
