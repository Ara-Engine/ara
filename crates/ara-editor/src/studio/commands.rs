//! The registry commands the studio runs, by id. Titles, icons, shortcuts and menus come
//! from the one registry (`shell::commands`), so the studio's menus, palette and keys match
//! the classic editor's and MCP's `editor.commands`; each runs through the same helpers
//! (`editor.*` actions, `cmd.apply`). Ids not ported yet show as unavailable with that
//! reason instead of doing nothing.
use serde_json::json;

use super::Studio;
use crate::shell::commands::Target;
use crate::shell::commands::table::{act, apply, play, play_params, select, set, view};

/// Why a registry command can't run in the studio yet, or `None` when it can.
pub(super) fn missing(id: &str) -> Option<&'static str> {
    (!ported(id))
        .then_some("Not in the studio yet: run the editor with --legacy-ui for this command")
}

/// A registry shortcut as this platform writes it: the first chord, with ⌘ ⇧ ⌥ on macOS
/// (`Ctrl+Shift+Z` reads `⇧⌘Z`); `None` when the command has none.
pub(super) fn shortcut(keys: &str) -> Option<String> {
    let first = keys.split(" / ").next().filter(|k| !k.is_empty())?;
    if !cfg!(target_os = "macos") {
        return Some(first.to_owned());
    }
    let mut mods = String::new();
    let mut rest = first;
    // Modifiers come in any order ("Alt+Shift+N").
    while let Some((r, glyph)) = [("Ctrl+", "⌘"), ("Shift+", "⇧"), ("Alt+", "⌥")]
        .into_iter()
        .find_map(|(name, glyph)| rest.strip_prefix(name).map(|r| (r, glyph)))
    {
        rest = r;
        mods.push_str(glyph);
    }
    // macOS orders modifiers ⌥⇧⌘.
    let order = |c: char| "⌃⌥⇧⌘".find(c).unwrap_or(0);
    let mut glyphs: Vec<char> = mods.chars().collect();
    glyphs.sort_by_key(|c| order(*c));
    let key = match rest {
        "Del" | "Delete" => "⌫",
        "Enter" => "↩",
        "Esc" => "⎋",
        other => other,
    };
    Some(glyphs.into_iter().collect::<String>() + key)
}

/// Whether the studio runs `id`.
pub(super) fn ported(id: &str) -> bool {
    super::create::item(id).is_some()
        || id.strip_prefix("assets.create.").is_some_and(|kind| {
            super::asset_create::KINDS
                .iter()
                .any(|(k, _, _)| *k == kind)
        })
        || matches!(
            id,
            "file.save"
                | "file.save_all"
                | "edit.undo"
                | "edit.redo"
                | "edit.duplicate"
                | "edit.delete"
                | "edit.rename"
                | "create.at_cursor"
                | "edit.preferences"
                | "layout.default"
                | "layout.level_design"
                | "layout.materials"
                | "layout.animation"
                | "layout.profiling"
                | "layout.laptop"
                | "layout.reset"
                | "window.hierarchy"
                | "window.inspector"
                | "window.assets"
                | "window.console"
                | "window.problems"
                | "window.agents"
                | "window.history"
                | "edit.project_settings"
                | "edit.select_all"
                | "edit.deselect"
                | "edit.palette"
                | "view.frame"
                | "view.frame_all"
                | "view.tool.move"
                | "view.tool.rotate"
                | "view.tool.scale"
                | "view.tool.universal"
                | "view.space"
                | "view.pivot"
                | "snap.toggle"
                | "view.theme.dark"
                | "view.theme.light"
                | "view.reduce_motion"
                | "play.start"
                | "play.pause"
                | "play.step"
                | "play.stop"
                | "view.camera.perspective"
                | "view.camera.default"
        )
}

/// Runs registry command `id` on `target`; failures are reported like any editor failure.
pub(super) fn run(s: &mut Studio, id: &str, target: &Target) {
    // Assets' Create menu, wherever it is asked for (context menus, the palette, agents).
    if let Some(kind) = id.strip_prefix("assets.create.")
        && super::asset_create::KINDS
            .iter()
            .any(|(k, _, _)| *k == kind)
    {
        super::asset_create::create(s, kind);
        return;
    }
    let e = &mut s.editor;
    let result = match id {
        "file.save" | "file.save_all" => {
            e.host.flush();
            Ok(())
        }
        "edit.undo" => act(e, "history.undo", json!({})),
        "edit.redo" => act(e, "history.redo", json!({})),
        "edit.duplicate" => {
            let entities = target.entities(&e.state);
            e.apply(
                vec![json!({"cmd":"editor.entities.duplicate","args":{"entities":entities}})],
                "Duplicate selection",
                None,
            )
            .map_err(Into::into)
            .and_then(|result| match result["outputs"][0]["roots"].as_array() {
                Some(roots) => act(e, "editor.select", json!({"entities": roots})),
                None => Ok(()),
            })
        }
        "edit.delete" => {
            let entities = target.entities(&e.state);
            apply(
                e,
                vec![json!({"cmd": "entity.delete", "args": {"entities": entities}})],
                "Delete selection",
            )
        }
        "create.group" => {
            let selected = target.entities(&e.state);
            match crate::shell::hierarchy_actions::group(e, &s.resolved, &selected) {
                Ok(parent) => select(e, vec![parent]),
                Err(error) => Err(error),
            }
        }
        "layout.default" => act(e, "editor.layout.apply", json!({"name": "default"})),
        "layout.level_design" => act(e, "editor.layout.apply", json!({"name": "levelDesign"})),
        "layout.materials" => act(e, "editor.layout.apply", json!({"name": "materials"})),
        "layout.animation" => act(e, "editor.layout.apply", json!({"name": "animation"})),
        "layout.profiling" => act(e, "editor.layout.apply", json!({"name": "profiling"})),
        "layout.laptop" => act(e, "editor.layout.apply", json!({"name": "laptop"})),
        "layout.reset" => act(e, "editor.layout.reset", json!({})),
        id if id.starts_with("window.") => {
            let panel = id.trim_start_matches("window.");
            // Tool pages open floating in the legacy window; the studio draws no floating
            // windows yet, so they dock in the centre beside the Scene.
            act(e, "editor.layout.show", json!({"panel": panel}))
                .and_then(|_| {
                    if e.state.floating.iter().any(|f| holds(&f.root, panel)) {
                        act(
                            e,
                            "editor.layout.move",
                            json!({"panel": panel, "slot": "view"}),
                        )?;
                    }
                    Ok(())
                })
                .and_then(|_| act(e, "editor.focus", json!({"panel": panel})))
        }
        "edit.preferences" => {
            super::preferences::open(s, crate::settings::Scope::User);
            Ok(())
        }
        "edit.project_settings" => {
            super::preferences::open(s, crate::settings::Scope::Project);
            Ok(())
        }
        "create.at_cursor" => {
            super::create::toggle(s, super::create::Anchor::Key("scene/view"), None);
            Ok(())
        }
        id if super::create::item(id).is_some() => {
            if let Some(item) = super::create::item(id) {
                // From an entity's menu (Create Child) it goes under that entity.
                let child = matches!(target, Target::Entity(_));
                super::create::create(s, item, target, None, child);
            }
            Ok(())
        }
        "edit.rename" => {
            let id = match target {
                Target::Entity(id) => Some(id.clone()),
                _ => e.state.selection.first().cloned(),
            };
            if let Some(id) = id {
                let name = s.name(&id);
                s.local.rename = Some((id, name));
                s.local.focus = Some("hierarchy/rename");
            }
            Ok(())
        }
        "edit.select_all" => {
            let all: Vec<String> = s.resolved.keys().cloned().collect();
            select(&mut s.editor, all)
        }
        "edit.deselect" => select(e, Vec::new()),
        "edit.palette" => {
            s.local.palette = Some((String::new(), 0));
            Ok(())
        }
        "view.frame" => act(e, "editor.frame", json!({})),
        "view.frame_all" => act(e, "editor.frame", json!({"all": true})),
        "view.tool.move" | "view.tool.rotate" | "view.tool.scale" | "view.tool.universal" => {
            let tool = match id {
                "view.tool.move" => "translate",
                "view.tool.rotate" => "rotate",
                "view.tool.scale" => "scale",
                _ => "universal",
            };
            view(e, json!({"gizmos": true}))
                .and_then(|()| act(e, "editor.tool", json!({"tool": tool})))
        }
        "view.space" => {
            let local = !e.state.local;
            act(
                e,
                "editor.tool",
                json!({"tool": e.state.tool, "local": local}),
            )
        }
        "view.pivot" => {
            let centre = !e.state.centre;
            act(
                e,
                "editor.tool",
                json!({"tool": e.state.tool, "centre": centre}),
            )
        }
        "snap.toggle" => {
            let on = !e.state.snapping;
            act(
                e,
                "editor.tool",
                json!({"tool": e.state.tool, "snapping": on}),
            )
        }
        "view.theme.dark" => set(e, "appearance.theme", json!("dark")),
        "view.theme.light" => set(e, "appearance.theme", json!("light")),
        "view.reduce_motion" => {
            let on = !e.state.reduce_motion;
            act(e, "editor.preferences", json!({"reduceMotion": on}))
        }
        "play.start" => {
            if e.state.playing == Some(false) {
                play(e, "play.resume", json!({}))
            } else {
                let mut params = play_params(e);
                params["scene"] = json!(e.state.doc);
                params["background"] = json!(true);
                // In the Game view (the default): an embedded player at its size, and the
                // Game tab comes forward, as in Unity.
                e.embed_play(&mut params);
                if e.plays_in_game_view() {
                    let _ = act(e, "editor.layout.show", json!({"panel": "game"}))
                        .and_then(|_| act(e, "editor.focus", json!({"panel": "game"})));
                }
                play(e, "play.start", params).and_then(|()| view(e, json!({"play": "playing"})))
            }
        }
        "play.pause" => {
            play(e, "play.pause", json!({})).and_then(|()| view(e, json!({"play": "paused"})))
        }
        "play.step" => play(e, "play.step", json!({"ticks": 1})),
        "play.stop" => {
            play(e, "play.stop", json!({})).and_then(|()| view(e, json!({"play": "stopped"})))
        }
        "view.camera.perspective" => act(
            e,
            "editor.camera",
            json!({"projection": if e.state.camera.projection == crate::state::CameraProjection::Perspective { "orthographic" } else { "perspective" }}),
        ),
        "view.camera.default" => act(e, "editor.frame", json!({"all": true})),
        _ => Err(format!("{id}: {}", missing(id).unwrap_or("unknown command")).into()),
    };
    if result.is_ok() && id.starts_with("create.") {
        s.resolve();
    }
    if let Err(error) = result {
        s.editor.report_failure(&*error);
    }
}

/// The registry spec of `id`, for titles, icons and shortcuts.
pub(super) fn spec(id: &str) -> Option<&'static crate::shell::commands::Spec> {
    crate::shell::commands::get(id)
}

/// The studio's keyboard map: the registry's default chords for the commands it runs.
pub(super) fn for_key(key: u16, mods: stillkit::el::Modifiers) -> Option<&'static str> {
    let c = mods.ctrl;
    let sh = mods.shift;
    Some(match (c, sh, key) {
        (true, false, 0x5A) => "edit.undo",
        (true, true, 0x5A) | (true, false, 0x59) => "edit.redo",
        (true, false, 0x53) => "file.save",
        (true, false, 0x44) => "edit.duplicate",
        (true, false, 0x41) => "edit.select_all",
        (true, true, 0x41) => "edit.deselect",
        (true, false, 0x4B) | (true, true, 0x50) => "edit.palette",
        (true, true, 0x4E) => "create.empty",
        (false, true, 0x41) => "create.at_cursor",
        (true, false, 0xBC) => "edit.preferences",
        (false, false, 0x2E) | (false, false, 0x08) => "edit.delete",
        (false, false, 0x46) => "view.frame",
        (false, false, 0x24) => "view.frame_all",
        (false, false, 0x57) => "view.tool.move",
        (false, false, 0x45) => "view.tool.rotate",
        (false, false, 0x52) => "view.tool.scale",
        (false, false, 0x54) => "view.tool.universal",
        (false, false, 0x74) => "play.start",
        (false, false, 0x75) => "play.pause",
        (false, false, 0x76) => "play.step",
        (false, true, 0x74) => "play.stop",
        _ => return None,
    })
}

/// Whether `panel` is a tab somewhere in `node`.
fn holds(node: &ara_ui::docking::DockNode, panel: &str) -> bool {
    use ara_ui::docking::DockNode;
    match node {
        DockNode::Area { tabs, .. } => tabs.iter().any(|t| t == panel),
        DockNode::Split { first, second, .. } => holds(first, panel) || holds(second, panel),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "macos")]
    fn shortcuts_read_as_macos_writes_them() {
        assert_eq!(super::shortcut("Ctrl+Shift+Z").as_deref(), Some("⇧⌘Z"));
        assert_eq!(super::shortcut("Alt+Shift+N").as_deref(), Some("⌥⇧N"));
        assert_eq!(super::shortcut("F2 / Enter").as_deref(), Some("F2"));
        assert_eq!(super::shortcut(""), None);
    }
}
