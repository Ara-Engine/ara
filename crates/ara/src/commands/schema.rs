//! `ara schema`: the Schema DB as text, as `reflection.json`, or as files.

use std::fs;
use std::path::Path;

use ara_schema::{SchemaDb, Shape, TypeDesc};
use serde_json::Value;

use super::print_text;
use crate::project::Project;

/// `reflection.json`: types, document kinds and the command catalog.
fn reflection(project: &Project) -> Value {
    let mut r = project.db.reflection();
    if let Some(o) = r.as_object_mut() {
        o.insert("commands".into(), project.registry.catalog());
    }
    r
}

/// Prints or writes the Schema DB.
pub fn schema(project: &Project, json: bool, out: Option<&Path>) -> Result<bool, String> {
    if !project.diags.is_empty() {
        print_text(&project.diags);
    }
    if let Some(dir) = out {
        let written = write_files(project, dir)?;
        println!("wrote {written} files to {}", dir.display());
    } else if json {
        print!("{}", ara_json::to_canonical_string(&reflection(project)));
    } else {
        print!("{}", summary(project));
    }
    Ok(!ara_diag::has_errors(&project.diags))
}

fn write_files(project: &Project, dir: &Path) -> Result<usize, String> {
    let types_dir = dir.join("schema").join("types");
    fs::create_dir_all(&types_dir)
        .map_err(|e| format!("cannot create `{}`: {e}", types_dir.display()))?;
    let mut files = vec![(dir.join("reflection.json"), reflection(project))];
    for kind in project.db.kinds() {
        files.push((
            dir.join("schema")
                .join(format!("{}.schema.json", kind.name)),
            project.db.doc_json_schema(kind),
        ));
    }
    for t in project.db.types() {
        files.push((
            types_dir.join(format!("{}.schema.json", t.name)),
            project.db.type_json_schema(t),
        ));
    }
    for (path, value) in &files {
        fs::write(path, ara_json::to_canonical_string(value))
            .map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
    }
    Ok(files.len())
}

/// A readable overview: one line per type with its fields indented.
fn summary(project: &Project) -> String {
    let db = &project.db;
    let mut s = String::new();
    for (title, kind) in [
        ("Components", "component"),
        ("Records", "record"),
        ("Value types", "struct"),
        ("Enums", "enum"),
    ] {
        let types: Vec<&TypeDesc> = db.types().filter(|t| t.kind_name() == kind).collect();
        if types.is_empty() {
            continue;
        }
        s.push_str(&format!("{title}\n"));
        for t in types {
            s.push_str(&type_line(db, t));
        }
        s.push('\n');
    }
    s.push_str("Documents\n");
    for k in db.kinds() {
        s.push_str(&format!(
            "  {:<10} {}  .{}  {}\n",
            k.name,
            k.schema_uri(),
            k.extension,
            k.doc
        ));
    }
    s.push_str("\nCommands\n");
    for info in project.registry.infos() {
        s.push_str(&format!(
            "  {:<24} {:<7} {}\n",
            info.name,
            format!("{:?}", info.kind).to_lowercase(),
            info.summary
        ));
    }
    s
}

fn type_line(db: &SchemaDb, t: &TypeDesc) -> String {
    let module = if t.module.is_empty() {
        String::new()
    } else {
        format!(" [{}]", t.module)
    };
    let mut s = format!("  {}{module}: {}\n", t.name, first_line(&t.doc));
    match &t.shape {
        Shape::Struct { fields, .. } => {
            for f in fields {
                let unit = f
                    .unit
                    .map(|u| format!(" {}", u.symbol()))
                    .unwrap_or_default();
                let default = if f.required {
                    " (required)".to_owned()
                } else {
                    format!(
                        " = {}",
                        ara_json::to_canonical_string(&db.field_default(f)).trim_end()
                    )
                };
                s.push_str(&format!(
                    "    {}: {}{default}{unit}  {}\n",
                    f.name,
                    f.ty,
                    first_line(&f.doc)
                ));
            }
        }
        Shape::Enum { variants } => {
            for v in variants {
                s.push_str(&format!("    {}  {}\n", v.name, first_line(&v.doc)));
            }
        }
    }
    s
}

/// The first paragraph of a doc comment.
fn first_line(doc: &str) -> &str {
    doc.split("\n\n").next().unwrap_or_default()
}
