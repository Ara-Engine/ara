//! `ara docs --llms`: writes `llms.txt`, `llms-full.txt` and one markdown page per
//! type, command and method for this engine version and project (offline, no lock).

use std::fs;
use std::path::Path;

use ara_host::Service;
use ara_host::docs;
use serde_json::json;

use crate::project::Project;

/// Writes the docs into `out` (default `<root>/.ara/docs`).
pub fn docs(project: &Project, out: Option<&Path>, json_out: bool) -> Result<bool, String> {
    let dir = out.map_or_else(|| project.root.join(".ara").join("docs"), Path::to_path_buf);
    let name = project.name();
    let mut methods = ara_host::core_methods();
    let service = ara_play_host::PlayService::new(
        &project.root,
        ara_play_host::PlayConfig::for_project(&project.root),
    )
    .with_headless_automation();
    let mut method_db = ara_schema::SchemaDb::new();
    let fixes = ara_play_host::SourceFixService::new(&project.root);
    methods.extend(
        service
            .methods(&mut method_db)
            .into_iter()
            .chain(fixes.methods(&mut method_db))
            .map(|m| {
                let service_name = if m.name == "source.fix" {
                    fixes.name()
                } else {
                    service.name()
                };
                ara_proto::MethodInfo {
                    params: method_db
                        .get(&m.params)
                        .map(|t| method_db.type_json_schema(t))
                        .unwrap_or_default(),
                    name: m.name,
                    summary: m.summary,
                    scope: m.scope,
                    hints: m.hints,
                    tool: m.tool,
                    service: service_name.into(),
                }
            }),
    );
    let pages = docs::generate(&project.db, &project.registry, &methods, &name);
    let mut files = vec![
        (
            "docs.json".to_owned(),
            docs::structured(&project.db, &project.registry, &methods, &name),
        ),
        ("llms.txt".to_owned(), docs::llms_txt(&pages, &name)),
        ("llms-full.txt".to_owned(), docs::llms_full(&pages, &name)),
    ];
    files.extend(pages.iter().map(|p| (format!("{}.md", p.id), p.markdown())));
    for (rel, text) in &files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create `{}`: {e}", parent.display()))?;
        }
        fs::write(&path, text).map_err(|e| format!("cannot write `{}`: {e}", path.display()))?;
    }
    if json_out {
        print!(
            "{}",
            ara_json::to_canonical_string(&json!({
                "out": dir.display().to_string(),
                "files": files.len(),
                "engine": docs::ENGINE_VERSION,
            }))
        );
    } else {
        println!(
            "wrote {} files to {} (ara {})",
            files.len(),
            dir.display(),
            docs::ENGINE_VERSION
        );
    }
    Ok(true)
}
