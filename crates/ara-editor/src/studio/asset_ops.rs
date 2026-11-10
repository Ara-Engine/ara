//! What Assets' items do, from their right-click menu, the palette, keys or agents (the
//! registry's `assets.*` commands): open, show in Finder, copy a reference or path,
//! reimport, rename in place, duplicate, delete after confirming, show the asset's
//! properties (the asset inspector), refresh. File changes are authored transactions on the
//! project (`cmd.apply` on `ara.project.json`, the legacy window's plans) and show in Assets
//! at once, before the host's next scan.
use serde_json::json;

use super::Studio;
use crate::catalog::Target;
use crate::shell::assets::Asset;

/// The `assets.*` commands the studio runs.
pub(super) const COMMANDS: [&str; 9] = [
    "assets.open",
    "assets.reveal",
    "assets.copy_reference",
    "assets.references",
    "assets.reimport",
    "assets.refresh",
    "assets.rename",
    "assets.duplicate",
    "assets.delete",
];

/// The asset a command acts on: its target, else the one picked in Assets.
fn target(s: &Studio, target: &Target) -> Option<Asset> {
    let path = match target {
        Target::Asset(p) | Target::Folder(p) => p.clone(),
        _ => s.editor.state.asset_path.clone()?,
    };
    let browser = s.assets.as_ref()?;
    browser
        .items
        .iter()
        .find(|a| a.path.trim_end_matches('/') == path.trim_end_matches('/'))
        .cloned()
        .or_else(|| Some(Asset::folder(path, None)))
}

/// Runs `id` (one of [`COMMANDS`]) on `target`.
pub(super) fn run(s: &mut Studio, id: &str, target: &Target) {
    let result = match (id, self::target(s, target)) {
        ("assets.refresh", _) => {
            if let Some(b) = s.assets.as_ref() {
                b.refresh();
            }
            Ok(())
        }
        (_, None) => Err(anyhow::anyhow!("Pick an asset first")),
        ("assets.open", Some(a)) => {
            open(s, &a);
            Ok(())
        }
        ("assets.reveal", Some(a)) => crate::shell::ops::open_external(&reveal_target(s, &a))
            .map_err(|e| anyhow::anyhow!("{e}")),
        ("assets.copy_reference", Some(a)) => {
            let workspace = s.editor.host.store().workspace();
            let text = a
                .current_reference(workspace)
                .or_else(|| a.authored_path.as_ref().map(|p| format!("@{p}")))
                .unwrap_or_else(|| a.path.clone());
            s.local.copy = Some(text);
            Ok(())
        }
        ("assets.references", Some(a)) => {
            s.act("editor.workspace", json!({"asset": a.path}));
            s.local.inspect_asset = true;
            Ok(())
        }
        ("assets.reimport", Some(a)) => {
            s.act("editor.asset.reimport", json!({"path": a.path}));
            Ok(())
        }
        ("assets.rename", Some(a)) => {
            let name = match a.kind.as_str() {
                "cs" => a.name().trim_end_matches(".cs"),
                _ => a.name(),
            };
            s.local.asset_rename = Some((a.path.clone(), name.to_owned()));
            s.local.focus = Some("asset/rename");
            Ok(())
        }
        ("assets.duplicate", Some(a)) => duplicate(s, a),
        ("assets.delete", Some(a)) => {
            s.local.asset_delete = Some(a.path);
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(error) = result {
        s.editor.report_failure(&*error);
    }
}

/// Opens an asset where it is edited: a folder in Assets, a scene in the Scene view, a
/// material, effect or controller in its page, anything else with the system's app.
pub(super) fn open(s: &mut Studio, a: &Asset) {
    if a.kind == "folder" {
        s.act(
            "editor.workspace",
            json!({"folder": a.path.trim_end_matches('/')}),
        );
        return;
    }
    if a.kind == "cs" {
        open_in_ide(s, &a.path, None);
        return;
    }
    if a.document_kind.as_deref() == Some("scene")
        && let Some(doc) = &a.authored_path
    {
        s.act("editor.scene.open", json!({"scene": doc}));
        s.resolve();
        s.act("editor.focus", json!({"panel": "scene"}));
        return;
    }
    let page = match (a.domain.as_deref(), a.kind.as_str()) {
        (Some("material"), _) => Some("material"),
        (Some("vfx"), _) => Some("vfx"),
        (Some("animation"), _) | (_, "animation") => Some("animation"),
        _ => None,
    };
    let workspace = s.editor.host.store().workspace();
    let doc = a
        .authored_path
        .as_deref()
        .and_then(|p| workspace.find(p))
        .map(|id| id.to_string());
    match (page, doc) {
        (Some(page), Some(doc)) => {
            s.act(
                "editor.workspace",
                json!({"resource": doc, "asset": a.path}),
            );
            super::commands::run(s, &format!("window.{page}"), &Target::None);
        }
        _ => {
            if let Err(error) =
                crate::shell::ops::open_external(&s.editor.host.root().join(&a.path))
            {
                s.editor.report_failure(&*error);
            }
        }
    }
}

/// Imports external `files` into storage folder `folder` (else the folder Assets shows),
/// as Unity imports a drop into the open folder. The batch is checked first; files that
/// already exist there wait for Rename, Replace or Skip.
pub(super) fn import(s: &mut Studio, files: Vec<std::path::PathBuf>, folder: Option<&str>) {
    let storage = folder.map_or_else(|| s.editor.state.asset_folder.clone(), str::to_owned);
    let destination = match s.assets.as_ref() {
        Some(b) => b.authored_folder_ref(&storage).map(str::to_owned),
        None => Err("Assets are still being read".to_owned()),
    };
    let result = destination
        .map_err(Into::into)
        .and_then(|destination| s.importer.import(&mut s.editor, files, destination));
    if let Err(error) = result {
        s.editor.report_failure(&*anyhow::anyhow!("{error}"));
    }
}

/// What Show in Finder opens: a folder itself, a file's folder.
fn reveal_target(s: &Studio, a: &Asset) -> std::path::PathBuf {
    let root = s.editor.host.root();
    if a.kind == "folder" {
        root.join(a.path.trim_end_matches('/'))
    } else {
        root.join(a.path.rsplit_once('/').map_or("", |(dir, _)| dir))
    }
}

/// Runs an authored file transaction on the project (one undo step). File work runs on
/// a host job; the host loop follows it ([`poll_jobs`]).
fn transact(s: &mut Studio, commands: Vec<serde_json::Value>, why: &str) -> anyhow::Result<()> {
    let reply = s.editor.action(
        "cmd.apply",
        json!({"doc": "ara.project.json", "commands": commands, "why": why}),
    )?;
    if let Some(job) = crate::asset_import::backend::accepted_job(&reply) {
        s.local.asset_jobs.push((job.to_owned(), why.to_owned()));
    }
    Ok(())
}

/// Follows file operations' jobs: one that ends rescans Assets (so what moved shows where
/// it went), one that fails says why. Whether any ended.
pub(super) fn poll_jobs(s: &mut Studio) -> bool {
    let mut ended = false;
    let jobs = std::mem::take(&mut s.local.asset_jobs);
    for (job, why) in jobs {
        let status = s.editor.action("jobs.status", json!({"job": job}));
        let status = match status {
            Ok(raw) => crate::asset_import::backend::Status::read(raw),
            Err(error) => {
                s.editor.report_failure(&error);
                ended = true;
                continue;
            }
        };
        if status.running() {
            s.local.asset_jobs.push((job, why));
            continue;
        }
        ended = true;
        if !status.succeeded() {
            s.editor
                .report_failure(&*anyhow::anyhow!("{why}: {}", status.summary()));
        }
    }
    if ended && let Some(b) = s.assets.as_ref() {
        b.refresh();
    }
    ended
}

fn duplicate(s: &mut Studio, a: Asset) -> anyhow::Result<()> {
    let inventory = s
        .assets
        .as_ref()
        .map(|b| b.items.clone())
        .unwrap_or_default();
    let plan = crate::shell::asset_ops::plan::duplicate(vec![a], &inventory)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    transact(s, plan.calls, &plan.title)?;
    if let Some(b) = s.assets.as_ref() {
        b.refresh();
    }
    Ok(())
}

/// Commits an in-place rename: the file (or folder) moves to its new name beside itself, its
/// references updated in the same transaction. Shows at once.
pub(super) fn rename(s: &mut Studio, path: &str, name: &str) {
    s.local.asset_rename = None;
    let name = name.trim();
    let Some(a) = target(s, &Target::Asset(path.to_owned())) else {
        return;
    };
    if a.kind == "cs" {
        rename_script(s, &a.path, name.trim_end_matches(".cs"));
        return;
    }
    if name.is_empty() || name == a.name() {
        return;
    }
    let result = (|| -> anyhow::Result<()> {
        let source =
            crate::shell::asset_ops::plan::source(&a).map_err(|e| anyhow::anyhow!("{e}"))?;
        let destination = match source.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{name}"),
            None => name.to_owned(),
        };
        transact(
            s,
            vec![
                json!({"cmd": "asset.move", "args": {"source": source, "destination": destination}}),
            ],
            &format!("Rename {} to {name}", a.name()),
        )?;
        if let Some(b) = s.assets.as_mut() {
            let storage = match a.path.trim_end_matches('/').rsplit_once('/') {
                Some((dir, _)) => format!("{dir}/{name}"),
                None => name.to_owned(),
            };
            b.remove_path(&a.path);
            b.insert_path(&storage, &a.kind);
            b.refresh();
        }
        Ok(())
    })();
    if let Err(error) = result {
        s.editor.report_failure(&*error);
    }
}

/// Moves `path` (a file or folder) into storage folder `folder`, its references updated in
/// the same transaction; shows at once.
pub(super) fn move_to(s: &mut Studio, path: &str, folder: &str) {
    s.local.assets_view.moving = None;
    let Some(a) = target(s, &Target::Asset(path.to_owned())) else {
        return;
    };
    let result = (|| -> anyhow::Result<()> {
        let destination = s
            .assets
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Assets are still being read"))?
            .authored_folder_ref(folder)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .to_owned();
        let plan = crate::shell::asset_ops::plan::move_to(vec![a.clone()], &destination)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        transact(s, plan.calls, &plan.title)?;
        if let Some(b) = s.assets.as_mut() {
            let name = a
                .path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&a.path);
            b.remove_path(&a.path);
            b.insert_path(
                &match folder {
                    "" => name.to_owned(),
                    folder => format!("{folder}/{name}"),
                },
                &a.kind,
            );
            b.refresh();
        }
        Ok(())
    })();
    if let Err(error) = result {
        s.editor.report_failure(&*error);
    }
}

/// Renames script `path` to `stem`.cs: its class with it, then compiles it (attaching it to
/// the entities New Script was asked from), as the shell's inline naming does.
fn rename_script(s: &mut Studio, path: &str, stem: &str) {
    let attach = std::mem::take(&mut s.local.script_attach);
    let to = match path.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{stem}.cs"),
        None => format!("{stem}.cs"),
    };
    let reply = s.editor.action(
        "script.rename",
        json!({"path": path, "to": to, "attachTo": attach, "doc": s.editor.state.doc}),
    );
    match reply {
        Ok(reply) => {
            if let Some(b) = s.assets.as_mut() {
                b.remove_path(path);
                b.insert_path(&to, "cs");
            }
            s.act("editor.workspace", json!({"asset": to}));
            if let Some(job) = reply["job"].as_str() {
                s.local
                    .asset_jobs
                    .push((job.to_owned(), format!("Compile {stem}.cs")));
            }
        }
        Err(error) => s.editor.report_failure(&error),
    }
}

/// Opens `path` in the configured IDE, at `location` (line, column) when given.
pub(super) fn open_in_ide(s: &mut Studio, path: &str, location: Option<(u32, u32)>) {
    if let Err(error) = crate::ide::open(&s.editor, std::path::Path::new(path), location) {
        s.editor.report_failure(&*anyhow::anyhow!("{error}"));
    }
}

/// Deletes `path` after its confirmation (referenced files are protected by the host).
pub(super) fn delete(s: &mut Studio, path: &str) {
    s.local.asset_delete = None;
    let Some(a) = target(s, &Target::Asset(path.to_owned())) else {
        return;
    };
    let result = crate::shell::asset_ops::plan::delete(vec![a.clone()])
        .map_err(|e| anyhow::anyhow!("{e}"))
        .and_then(|plan| transact(s, plan.calls, &plan.title));
    match result {
        Ok(()) => {
            if let Some(b) = s.assets.as_mut() {
                b.remove_path(&a.path);
                b.refresh();
            }
            if s.editor.state.asset_path.as_deref() == Some(a.path.as_str()) {
                s.local.inspect_asset = false;
            }
        }
        Err(error) => s.editor.report_failure(&*error),
    }
}
