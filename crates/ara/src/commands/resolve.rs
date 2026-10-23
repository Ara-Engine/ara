//! `ara resolve <file>`: settles the `$conflicts` an `ara merge` left in a document,
//! without hand-editing JSON. `--ours` / `--theirs` pick a side for every conflict;
//! `--pick <record>:<path>=ours|theirs|base` picks per conflict (`e_b2:name=theirs`;
//! `<record>=…` for a whole-record conflict). Unpicked conflicts stay and exit 1.

use std::path::Path;

use serde_json::{Value, json};

use super::edit::DocFile;
use crate::project::Project;

/// The side a conflict is settled with.
fn side_name(s: &str) -> Result<&'static str, String> {
    match s {
        "ours" => Ok("ours"),
        "theirs" => Ok("theirs"),
        "base" => Ok("base"),
        other => Err(format!("`{other}`: pick `ours`, `theirs` or `base`")),
    }
}

/// `record:path` of a conflict entry, as `--pick` names it.
fn key(entry: &Value) -> String {
    let record = entry["record"].as_str().unwrap_or_default();
    match entry["path"].as_str().filter(|p| !p.is_empty()) {
        Some(p) => format!("{record}:{p}"),
        None => record.to_owned(),
    }
}

/// Resolves conflicts in `file`; `all` is the side for unpicked conflicts.
pub fn resolve(
    project: &Project,
    file: &Path,
    all: Option<&str>,
    picks: &[String],
    json_out: bool,
) -> Result<bool, String> {
    let mut picked: Vec<(String, &'static str)> = Vec::new();
    for p in picks {
        let (k, side) = p
            .rsplit_once('=')
            .ok_or_else(|| format!("`--pick {p}`: expected `<record>:<path>=ours|theirs|base`"))?;
        picked.push((k.to_owned(), side_name(side)?));
    }
    let all = all.map(side_name).transpose()?;
    let mut doc = DocFile::read(file)?;
    let entries = match doc.root.shift_remove("$conflicts") {
        Some(Value::Array(e)) => e,
        _ => Vec::new(),
    };
    let (mut resolved, mut left) = (Vec::new(), Vec::new());
    for entry in entries {
        let k = key(&entry);
        let side = picked
            .iter()
            .find(|(p, _)| *p == k)
            .map(|(_, s)| *s)
            .or(all);
        let Some(side) = side else {
            left.push(entry);
            continue;
        };
        let record = entry["record"].as_str().unwrap_or_default();
        let path = entry["path"].as_str().unwrap_or_default();
        if !doc.set(record, path, entry.get(side).cloned()) {
            return Err(format!(
                "`{k}`: the record or path is not in `{}`",
                project.rel(file)
            ));
        }
        resolved.push(json!({ "conflict": k, "kept": side }));
    }
    if let Some((p, _)) = picked
        .iter()
        .find(|(p, _)| !resolved.iter().any(|r| r["conflict"] == *p))
    {
        return Err(format!("`--pick {p}`: no such conflict"));
    }
    let remaining: Vec<String> = left.iter().map(key).collect();
    if !left.is_empty() {
        doc.root
            .insert("$conflicts".into(), Value::Array(left.clone()));
    }
    doc.write(project, file)?;
    let rel = project.rel(file);
    if json_out {
        print!(
            "{}",
            ara_json::to_canonical_string(
                &json!({ "file": rel, "resolved": resolved, "remaining": left })
            )
        );
    } else {
        for r in &resolved {
            println!(
                "{}: kept {}",
                r["conflict"].as_str().unwrap_or_default(),
                r["kept"].as_str().unwrap_or_default()
            );
        }
        for (k, entry) in remaining.iter().zip(&left) {
            println!(
                "{k}: still conflicted (ours {}, theirs {}); `ara resolve {rel} --pick {k}=ours|theirs`",
                entry.get("ours").unwrap_or(&Value::Null),
                entry.get("theirs").unwrap_or(&Value::Null)
            );
        }
        println!(
            "{rel}: {} resolved, {} left",
            resolved.len(),
            remaining.len()
        );
    }
    Ok(remaining.is_empty())
}
