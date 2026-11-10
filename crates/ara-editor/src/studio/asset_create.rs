//! Creating assets in the open Assets folder, as the Add menu, an empty tool page or an agent
//! asks: a folder, a scene, a material / effect / animation controller (opened in its page),
//! or a C# script. Names never collide: the first free `name-N` is taken. The browser lists
//! storage paths; documents are created at authored (logical) paths.
use serde_json::json;

use super::Studio;

/// What the Add menu creates, in order: id, title, icon.
pub(super) const KINDS: [(&str, &str, stillkit::icons::Icon); 7] = [
    ("folder", "Folder", ara_kit::icons::FOLDER),
    ("scene", "Scene", ara_kit::icons::CLAPPERBOARD),
    ("material", "Material", ara_kit::icons::PALETTE),
    ("vfx", "VFX Effect", ara_kit::icons::SPARKLES),
    (
        "animation",
        "Animation Controller",
        ara_kit::icons::PERSON_STANDING,
    ),
    ("script", "C# Script", ara_kit::icons::FILE_CODE),
    ("system", "C# System", ara_kit::icons::FILE_CODE),
];

/// The open Assets folder as an authored (logical) path; the project root when unknown.
fn authored_folder(s: &Studio) -> String {
    let storage = &s.editor.state.asset_folder;
    s.assets
        .as_ref()
        .and_then(|b| b.authored_folder_ref(storage).ok())
        .unwrap_or("")
        .to_owned()
}

/// The first free `stem-N.extension` (or `stem` alone for folders) under the open folder.
fn free_path(s: &Studio, stem: &str, extension: &str) -> Option<String> {
    let folder = authored_folder(s);
    let workspace = s.editor.host.store().workspace();
    let taken = |path: &str| {
        workspace.find(path).is_some()
            || s.assets.as_ref().is_some_and(|b| {
                b.items
                    .iter()
                    .any(|a| a.authored_path.as_deref() == Some(path))
            })
    };
    (1..=10_000)
        .map(|n| {
            let leaf = match (n, extension) {
                (1, "") => stem.to_owned(),
                (_, "") => format!("{stem} {n}"),
                (_, ext) => format!("{stem}-{n}.{ext}"),
            };
            if folder.is_empty() {
                leaf
            } else {
                format!("{folder}/{leaf}")
            }
        })
        .find(|path| !taken(path))
}

/// Creates `kind` (one of [`KINDS`]) in the open folder, and opens it where it is edited.
pub(super) fn create(s: &mut Studio, kind: &str) {
    let result = match kind {
        "folder" => folder(s),
        "scene" => resource(s, "scene", "untitled", "scene"),
        "material" => resource(s, "material", "material", "graph"),
        "vfx" => resource(s, "vfx", "vfx", "graph"),
        "animation" => resource(s, "animation", "animation", "animation"),
        "script" | "system" => script(s, kind),
        _ => Ok(()),
    };
    if let Err(error) = result {
        s.editor.report_failure(&*error);
    }
}

fn folder(s: &mut Studio) -> anyhow::Result<()> {
    let path =
        free_path(s, "New Folder", "").ok_or_else(|| anyhow::anyhow!("no free folder name"))?;
    s.editor.action(
        "cmd.apply",
        json!({"doc": "ara.project.json", "commands": [{"cmd": "folder.create", "args": {"path": path}}], "why": "Create authored folder"}),
    )?;
    show(s, &path, "folder");
    Ok(())
}

/// Shows a just-created `path` (authored) in Assets at once, before the host's next scan.
fn show(s: &mut Studio, path: &str, kind: &str) {
    let workspace = s.editor.host.store().workspace();
    let Some(browser) = s.assets.as_mut() else {
        return;
    };
    if let Ok(storage) = crate::shell::ops::authored::storage(&browser.files, workspace, path) {
        browser.insert_path(&storage, kind);
    }
    browser.refresh();
}

/// A new `domain` document, opened: a scene in the Scene view, a graph or controller in its
/// tool page.
fn resource(s: &mut Studio, domain: &str, stem: &str, extension: &str) -> anyhow::Result<()> {
    let path = free_path(s, stem, extension).ok_or_else(|| anyhow::anyhow!("no free name"))?;
    let result = s.editor.action(
        "editor.resource.create",
        json!({"path": path, "domain": domain}),
    )?;
    let doc = result["doc"].clone();
    if domain == "scene" {
        s.act("editor.scene.open", json!({"scene": doc}));
        s.resolve();
        s.act("editor.focus", json!({"panel": "scene"}));
    } else {
        s.act("editor.workspace", json!({"resource": doc, "asset": ""}));
        super::commands::run(
            s,
            &format!("window.{domain}"),
            &crate::catalog::Target::None,
        );
    }
    show(s, &path, extension);
    Ok(())
}

/// A behaviour (or batched `system`) script from Ara's template, selected in Assets.
fn script(s: &mut Studio, kind: &str) -> anyhow::Result<()> {
    let (stem, template) = if kind == "system" {
        ("NewSystem", "system")
    } else {
        ("NewBehaviour", "behaviour")
    };
    let folder = authored_folder(s);
    let taken = |name: &str| {
        s.assets.as_ref().is_some_and(|b| {
            b.items
                .iter()
                .any(|a| a.kind == "cs" && a.path.rsplit('/').next() == Some(name))
        })
    };
    let name = (1..=10_000)
        .map(|n| {
            if n == 1 {
                format!("{stem}.cs")
            } else {
                format!("{stem}{n}.cs")
            }
        })
        .find(|name| !taken(name))
        .ok_or_else(|| anyhow::anyhow!("no free script name"))?;
    let logical = if folder.is_empty() {
        name
    } else {
        format!("{folder}/{name}")
    };
    let storage = match &s.assets {
        Some(b) => crate::shell::ops::authored::storage(
            &b.files,
            s.editor.host.store().workspace(),
            &logical,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?,
        None => logical,
    };
    let created = s.editor.action(
        "script.create",
        json!({"path": storage, "template": template, "compile": false}),
    )?;
    if let Some(path) = created["path"].as_str() {
        if let Some(browser) = s.assets.as_mut() {
            browser.insert_path(path, "cs");
        }
        s.act("editor.workspace", json!({"asset": path}));
        s.local.inspect_asset = true;
    }
    Ok(())
}
