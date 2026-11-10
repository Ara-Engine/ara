//! Full project-index/load probe; scratch fixtures are isolated under workspace target/.
use ara_diag::{Diagnostic, codes};
use ara_doc::Workspace;
use ara_host::Project;
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    hint::black_box,
    path::{Path, PathBuf},
    time::Instant,
};

// Frozen pre-parallel path. These fixtures contain native scene/prefab/UI files
// and the project marker only, so marker equality identifies project-scope docs.
fn sequential(project: &Project) -> (Workspace, BTreeMap<String, Vec<Diagnostic>>) {
    let mut workspace = Workspace::new(project.index.clone());
    workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
    let mut diagnostics = BTreeMap::new();
    let mut seen = HashSet::new();
    for file in &project.docs {
        let relative = project.rel(file);
        if !seen.insert(relative.clone()) {
            continue;
        }
        match fs::read_to_string(file) {
            Ok(text) => {
                let result = project.read_document(&text, &relative);
                if let Some(doc) = result.doc {
                    if file == &project.root.join("ara.project.json") {
                        workspace.insert_project_document(relative.clone(), doc);
                    } else {
                        workspace.insert_with_asset_path(relative.clone(), doc, &relative);
                    }
                }
                diagnostics.insert(relative, result.diags);
            }
            Err(error) => {
                diagnostics.insert(
                    relative.clone(),
                    vec![
                        Diagnostic::error(codes::IO, format!("cannot read: {error}"))
                            .file(relative),
                    ],
                );
            }
        }
    }
    (workspace, diagnostics)
}

fn write(path: PathBuf, text: String) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
    fs::write(path, text).expect("fixture file");
}

fn fixture(root: &Path, pairs: usize, rows: usize, menus: usize, pinned_ui: bool) {
    write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(&json!({"layoutVersion":4,"name":"Loading probe"})),
    );
    for i in 0..pairs {
        for (kind, prefix) in [("scene", "sc"), ("prefab", "pf")] {
            let mut records: Map<String, Value> = (0..rows).map(|row| {
                let mut record = json!({"name":format!("Object {row}"),"Transform":{"position":[row,1,2],"rotation":[0,15,0],"scale":[1,1,1]}});
                if row > 0 { record["parent"] = "e_00000000".into(); }
                (format!("e_{row:08}"),record)
            }).collect();
            if kind == "scene" {
                records.insert(
                    "e_instance".into(),
                    json!({"prefab":format!("@prefabs/{i}.prefab#pf_{i:08}")}),
                );
            }
            let text = ara_json::to_canonical_string(
                &json!({"$schema":format!("ara://schema/{kind}/1"),"id":format!("{prefix}_{i:08}"),"records":records}),
            );
            write(
                root.join("content").join(format!(
                    "{}s/{i}.{kind}",
                    if kind == "scene" { "level" } else { "prefab" }
                )),
                text,
            );
        }
    }
    for menu in 0..menus {
        let mut text = if pinned_ui {
            format!("<!-- ara:asset ui_{menu:08} -->\n")
        } else {
            String::new()
        };
        text.push_str(&format!(
            "<ui id='root'><text id='title' text='Menu {menu}'/>"
        ));
        for i in 0..32 {
            text.push_str(&format!(
                "<panel id='row{i}'><text id='label{i}' text='Item {i}'/></panel>"
            ));
        }
        text.push_str("</ui>");
        write(root.join(format!("content/ui/menu{menu}.ui")), text);
    }
}

fn measure<T>(name: &str, mut run: impl FnMut() -> T) {
    for _ in 0..3 {
        black_box(run());
    }
    let mut samples = Vec::with_capacity(12);
    for _ in 0..12 {
        let start = Instant::now();
        let value = black_box(run());
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        drop(value);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"case":name,"p50_ms":(samples[5]+samples[6])*0.5,"p95_ms":samples[11],"samples_ms":samples})
    );
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/iteration-perf")
        .join(format!("project-load-{}", std::process::id()));
    let reverse = std::env::args().any(|arg| arg == "--reverse");
    let open_only = std::env::args().any(|arg| arg == "--open-only");
    for (name, pairs, rows, menus) in [
        ("small", 2, 32, 1),
        ("many_tiny", 32, 1, 0),
        ("ui_heavy", 2, 32, 256),
        ("ui_pinned", 2, 32, 256),
        ("medium", 32, 128, 8),
        ("large", 128, 128, 16),
    ] {
        let case = root.join(name);
        fixture(&case, pairs, rows, menus, name == "ui_pinned");
        let project = Project::open(&case).expect("project opens");
        let (workspace, diags) = project.load(&[]);
        let errors: Vec<_> = project
            .diags
            .iter()
            .chain(diags.values().flatten())
            .filter(|d| d.is_error())
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:?}");
        assert_eq!(workspace.docs().count(), pairs * 2 + menus + 1);
        let reference = sequential(&project);
        assert_eq!(reference.1, diags);
        let written = |workspace: &Workspace| {
            workspace
                .docs()
                .map(|(path, doc)| {
                    (
                        path.to_owned(),
                        project.write_document(doc).expect("native document"),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(written(&reference.0), written(&workspace));
        drop(reference);
        drop(workspace);
        println!(
            "{}",
            json!({"case":name,"documents":project.docs.len(),"records":pairs*2*rows+pairs,"ui_documents":menus})
        );
        measure(&format!("{name}_open"), || {
            Project::open(&case).expect("project opens")
        });
        if open_only {
            continue;
        }
        for baseline in if reverse {
            [false, true]
        } else {
            [true, false]
        } {
            let suffix = if baseline { "reference" } else { "current" };
            measure(&format!("{name}_load_{suffix}"), || {
                if baseline {
                    sequential(&project)
                } else {
                    project.load(&[])
                }
            });
            measure(&format!("{name}_open_load_{suffix}"), || {
                let project = Project::open(&case).expect("project opens");
                let loaded = if baseline {
                    sequential(&project)
                } else {
                    project.load(&[])
                };
                (project, loaded)
            });
        }
    }
}
