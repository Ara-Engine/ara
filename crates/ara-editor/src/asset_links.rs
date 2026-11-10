//! What an asset uses and what uses it: dependency links over browser paths and
//! the logical/physical path mapping they need.
use crate::assets;
use serde_json::Value;
pub(crate) mod namespace;

/// What the asset at `path` uses and what uses it, as browser paths: the browser's
/// filesystem snapshot, overridden by the live documents (authoring ops land first).
pub(crate) fn links(
    browser: &assets::Browser,
    workspace: &ara_doc::Workspace,
    path: &str,
) -> (Vec<String>, Vec<String>) {
    let mut graph = ara_asset::DependencyGraph::default();
    for item in &browser.items {
        graph.replace(
            item.path.clone(),
            item.uses.iter().filter_map(|reference| {
                namespace::dependency(&browser.files, workspace, reference)
            }),
        );
    }
    for (doc_path, doc) in workspace.docs() {
        let mut refs = Vec::new();
        for record in doc.records().values() {
            for value in record.values() {
                collect_refs(value, &mut refs);
            }
        }
        let source = if doc.kind == "import" {
            doc_path.strip_suffix(".import.json").unwrap_or(doc_path)
        } else {
            doc_path
        };
        graph.replace(
            source.to_owned(),
            refs.iter().filter_map(|reference| {
                namespace::dependency(&browser.files, workspace, reference)
            }),
        );
    }
    (
        graph.dependencies(path).map(str::to_owned).collect(),
        graph.users(path).map(str::to_owned).collect(),
    )
}

fn collect_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) if s.starts_with('@') => out.push(s.clone()),
        Value::Array(a) => {
            for v in a {
                collect_refs(v, out)
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                collect_refs(v, out)
            }
        }
        _ => {}
    }
}
