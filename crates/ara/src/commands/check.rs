//! `ara check`: reader diagnostics (schema, references, structure) plus prefab
//! instance checks that need other documents. `--fix` first applies every
//! diagnostic that has exactly one `rename`, `set` or `remove` fix, then checks
//! again.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ara_diag::{Diagnostic, FixAction};
use ara_doc::AssetId;

use super::edit::DocFile;
use super::{json_report, print_text, tally};
use crate::project::Project;

/// The diagnostics of `files` (every project document if empty).
fn diagnostics(project: &Project, files: &[PathBuf]) -> (Vec<PathBuf>, Vec<Diagnostic>) {
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
    (targets, diags)
}

/// Applies the unambiguous fixes; returns their titles (`file: title`).
fn apply_fixes(project: &Project, diags: &[Diagnostic]) -> Result<Vec<String>, String> {
    let mut by_file: BTreeMap<&str, Vec<&Diagnostic>> = BTreeMap::new();
    for d in diags {
        if let (Some(file), [_]) = (d.location.file.as_deref(), d.fixes.as_slice()) {
            by_file.entry(file).or_default().push(d);
        }
    }
    let mut done = Vec::new();
    for (file, list) in by_file {
        let path = project.root.join(file);
        let Ok(mut doc) = DocFile::read(&path) else {
            continue;
        };
        let mut changed = false;
        for d in list {
            let fix = &d.fixes[0];
            let at = |record: &Option<String>| record.clone().or_else(|| d.location.record.clone());
            let ok = match &fix.action {
                FixAction::Rename { record, path, to } => {
                    at(record).is_some_and(|r| doc.rename(&r, path, to))
                }
                FixAction::Set {
                    record,
                    path,
                    value,
                } => at(record).is_some_and(|r| doc.set(&r, path, Some(value.clone()))),
                FixAction::Remove { record, path } => {
                    at(record).is_some_and(|r| doc.set(&r, path, None))
                }
                _ => false,
            };
            if ok {
                done.push(format!("{file}: {}", fix.title));
                changed = true;
            }
        }
        if changed {
            doc.write(project, &path)?;
        }
    }
    Ok(done)
}

/// Checks `files` (every project document if empty); with `fix`, applies the
/// unambiguous fixes first.
pub fn check(project: &Project, files: &[PathBuf], fix: bool, json: bool) -> Result<bool, String> {
    let (mut targets, mut diags) = diagnostics(project, files);
    let mut fixed = Vec::new();
    if fix {
        fixed = apply_fixes(project, &diags)?;
        if !fixed.is_empty() {
            let again = Project::open(&project.root).map_err(|d| d.to_string())?;
            (targets, diags) = diagnostics(&again, files);
        }
    }
    let (errors, warnings) = tally(&diags);
    if json {
        let mut report = json_report(targets.len(), &diags);
        if fix {
            report["fixed"] = fixed.into();
        }
        print!("{}", ara_json::to_canonical_string(&report));
    } else {
        for f in &fixed {
            println!("fixed {f}");
        }
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
