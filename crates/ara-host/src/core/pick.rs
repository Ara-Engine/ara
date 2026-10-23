//! Which document a call is about.
//!
//! - Named: by asset ID, path or `@path#id`, and leniently by a unique path without
//!   its extension (`levels/outpost`), file name (`outpost.scene`) or stem
//!   (`outpost`).
//! - Not named: the one document holding every entity the call addresses (IDs,
//!   addresses, name selectors), else the only scene, else the project's main
//!   scene. Otherwise `ARA1814` with a fix per candidate document.

use ara_cmd::select::{Selector, find};
use ara_diag::{Diagnostic, FixAction, codes, did_you_mean};
use ara_doc::AssetId;

use crate::host::State;

fn stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.split_once('.').map_or(file, |(s, _)| s)
}

/// A loaded document by asset ID, path, `@path#id` or a unique shorthand; with a
/// did-you-mean.
pub(crate) fn find_doc(st: &State, key: &str) -> Result<AssetId, Vec<Diagnostic>> {
    let ws = st.store.workspace();
    if let Some(id) = ws.find(key) {
        return Ok(id.clone());
    }
    let key_l = key.trim_start_matches("./").to_lowercase();
    let loose: Vec<&AssetId> = ws
        .docs()
        .filter(|(path, _)| {
            let p = path.to_lowercase();
            let file = p.rsplit('/').next().unwrap_or(&p);
            let no_ext = p.rsplit_once('.').map_or(p.as_str(), |(a, _)| a);
            key_l == p || key_l == no_ext || key_l == file || key_l == stem(&p)
        })
        .map(|(_, d)| &d.id)
        .collect();
    if let [one] = loose.as_slice() {
        return Ok((*one).clone());
    }
    let mut msg = format!("no document `{key}` is loaded");
    if let Some(near) = did_you_mean(key, ws.docs().map(|(p, _)| p)) {
        msg.push_str(&format!("; did you mean `{near}`?"));
    }
    Err(vec![Diagnostic::error(codes::DOC_NOT_FOUND, msg)])
}

/// The project-relative path of a loaded document.
pub(crate) fn path_of(st: &State, id: &AssetId) -> String {
    st.store
        .workspace()
        .path_of(id.as_str())
        .unwrap_or(id.as_str())
        .to_owned()
}

/// `explicit`, else the document `refs` (entity strings of the call) point into,
/// else the default scene.
pub(crate) fn pick(
    st: &State,
    explicit: Option<&str>,
    refs: &[String],
) -> Result<AssetId, Vec<Diagnostic>> {
    if let Some(key) = explicit {
        return find_doc(st, key);
    }
    let refs: Vec<Selector<'_>> = refs
        .iter()
        .filter(|r| !r.starts_with('$'))
        .map(|r| Selector::parse(r))
        .collect();
    if refs.is_empty() {
        return default_scene(st);
    }
    let ws = st.store.workspace();
    let db = st.store.schema();
    let holders: Vec<(&str, AssetId)> = ws
        .docs()
        .filter(|(_, doc)| {
            let mut ignored = Vec::new();
            let entities = ws.resolve(&doc.id, db, &mut ignored);
            refs.iter().all(|r| !find(&entities, *r).is_empty())
        })
        .map(|(path, doc)| (path, doc.id.clone()))
        .collect();
    match holders.as_slice() {
        [] => default_scene(st),
        [(_, id)] => Ok(id.clone()),
        many => Err(vec![choose(
            "these entities are in several documents; pass `doc`",
            many.iter().map(|(p, _)| *p),
        )]),
    }
}

/// The only scene, else the project's main scene.
fn default_scene(st: &State) -> Result<AssetId, Vec<Diagnostic>> {
    let ws = st.store.workspace();
    let scenes: Vec<(&str, &AssetId)> = ws
        .docs()
        .filter(|(_, d)| d.kind == "scene")
        .map(|(p, d)| (p, &d.id))
        .collect();
    if let [(_, id)] = scenes.as_slice() {
        return Ok((*id).clone());
    }
    if let Some(id) = st.main.as_deref().and_then(|m| ws.find(m)) {
        return Ok(id.clone());
    }
    Err(vec![choose(
        "no document given and the project has no main scene; pass `doc`",
        scenes.iter().map(|(p, _)| *p),
    )])
}

/// `ARA1814` with one `doc` fix per candidate path.
fn choose<'a>(msg: &str, candidates: impl Iterator<Item = &'a str>) -> Diagnostic {
    candidates.fold(
        Diagnostic::error(codes::DOC_REQUIRED, msg).path("doc"),
        |d, path| {
            d.with_fix(
                format!("Use `{path}`"),
                FixAction::Set {
                    record: None,
                    path: "doc".into(),
                    value: path.into(),
                },
            )
        },
    )
}
