//! The registry commands the studio runs, by id. Titles, icons, shortcuts and menus come
//! from the catalogue (`crate::catalog`), so the studio's menus, palette and keys match
//! MCP's `editor.commands`; each runs through `editor.*` actions or `cmd.apply`, and a
//! command a C# extension registered runs in the extension host.
use serde_json::json;

use super::Studio;
use crate::Editor;
use crate::catalog::Target;
use crate::shell::commands::table::play_params;
use serde_json::Value;

/// What running a command gives back: done, or why not.
type R = ara_ui::Result<()>;

/// Runs editor action `method`.
fn act(e: &mut Editor, method: &str, params: Value) -> R {
    e.action(method, params)?;
    Ok(())
}

/// Applies authoring `commands` as one undo step, `why` naming it; nothing to apply is
/// fine.
fn apply(e: &mut Editor, commands: Vec<Value>, why: &str) -> R {
    if !commands.is_empty() {
        e.apply(commands, why, None)?;
    }
    Ok(())
}

/// Sets user setting `key`.
fn set(e: &mut Editor, key: &str, value: Value) -> R {
    act(
        e,
        "editor.settings.set",
        json!({"key": key, "value": value}),
    )
}

/// Changes the view state (`editor.view`: view mode, gizmos, Play's state).
fn view(e: &mut Editor, params: Value) -> R {
    act(e, "editor.view", params)
}

/// Selects `entities`.
fn select(e: &mut Editor, entities: Vec<String>) -> R {
    act(e, "editor.select", json!({"entities": entities}))
}

/// Runs a `play.*` action on the running world (any but `play.start` names it).
fn play(e: &mut Editor, method: &str, mut params: Value) -> R {
    if method != "play.start"
        && let Some(world) = &e.play_world
    {
        params["world"] = json!(world);
    }
    act(e, method, params)
}

/// The asset a command acts on: its asset or folder target, else the selected entity's
/// model (as Assets lists it), else the asset picked in Assets.
fn asset_of(e: &Editor, target: &Target) -> Option<String> {
    match target {
        Target::Asset(p) | Target::Folder(p) => return Some(p.clone()),
        _ => {}
    }
    let model = target.entity(&e.state).and_then(|entity| {
        let resolved = crate::world::resolve(e.host.store(), &e.state.doc).ok()?;
        let reference = resolved
            .get(entity)?
            .record
            .get("MeshRenderer")?
            .get("model")?
            .as_str()?
            .to_owned();
        let path = reference.trim_start_matches('@');
        Some(path.split('#').next().unwrap_or(path).to_owned())
    });
    model.or_else(|| e.state.asset_path.clone())
}

/// Flips a user setting that is `default` when unset.
fn flip_setting(e: &mut Editor, key: &str, default: bool) -> R {
    let on = e
        .state
        .settings
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default);
    set(e, key, json!(!on))
}

/// `snake_case` as `camelCase` (the `editor.view` mode names).
fn camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `roots` and everything under them, roots first.
fn with_descendants(resolved: &ara_doc::prefab::Resolved, roots: &[String]) -> Vec<String> {
    let mut out: Vec<String> = roots.to_vec();
    let mut i = 0;
    while i < out.len() {
        let parent = out[i].clone();
        let children: Vec<String> = resolved
            .iter()
            .filter(|(id, x)| x.parent.as_deref() == Some(parent.as_str()) && !out.contains(id))
            .map(|(id, _)| id.clone())
            .collect();
        out.extend(children);
        i += 1;
    }
    out
}

/// A `@scene#entity` reference to `entity` in the open scene, as agents and fields use.
pub(super) fn reference(e: &Editor, entity: &str) -> String {
    let workspace = e.host.store().workspace();
    let path = workspace
        .path_of(&e.state.doc)
        .map(|path| workspace.reference_path(path))
        .unwrap_or("");
    format!("@{path}#{entity}")
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

/// Whether the editor runs `id` (it is in the catalogue).
pub(super) fn ported(id: &str) -> bool {
    crate::catalog::get(id).is_some()
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
    // A command a C# extension registered runs in the extension host.
    if crate::catalog::get(id).is_none()
        && let Some(command) = s.editor.extensions.commands.iter().find(|c| c.id == id)
    {
        let result = if command.enabled {
            s.editor
                .action("editor.command", json!({"id": id, "target": target}))
                .map(|_| ())
                .map_err(|e| e.to_string())
        } else {
            Err(format!(
                "{}: {}",
                command.title,
                command
                    .disabled_reason
                    .as_deref()
                    .unwrap_or("disabled until the C# extensions reload")
            ))
        };
        if let Err(error) = result {
            s.editor.report_failure(&*anyhow::anyhow!(error));
        }
        return;
    }
    if super::asset_ops::COMMANDS.contains(&id) {
        super::asset_ops::run(s, id, target);
        return;
    }
    let revision = s.editor.host.store().revision();
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
            if e.state.playing.is_none()
                && e.state
                    .settings
                    .get("game.maximizeOnPlay")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                let _ = view(e, json!({"maximized": "game"}));
            }
            let clear = e
                .state
                .settings
                .get("console.clearOnPlay")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if clear && e.state.playing.is_none() {
                s.logs.clear();
            }
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
            // A Game view maximized for Play comes back with the layout.
            let restore = e.state.maximized.as_deref() == Some("game")
                && e.state
                    .settings
                    .get("game.maximizeOnPlay")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            play(e, "play.stop", json!({}))
                .and_then(|()| view(e, json!({"play": "stopped"})))
                .and_then(|()| {
                    if restore {
                        view(e, json!({"maximized": ""}))
                    } else {
                        Ok(())
                    }
                })
        }
        id if id.starts_with("game.aspect.") => set(
            e,
            "game.aspect",
            json!(id.trim_start_matches("game.aspect.")),
        ),
        "game.maximize_on_play" => flip_setting(e, "game.maximizeOnPlay", false),
        "game.stats" => flip_setting(e, "game.stats", false),
        "view.camera.perspective" => act(
            e,
            "editor.camera",
            json!({"projection": if e.state.camera.projection == crate::state::CameraProjection::Perspective { "orthographic" } else { "perspective" }}),
        ),
        "select.children" => {
            let parents = target.entities(&e.state);
            let kids = s
                .resolved
                .iter()
                .filter(|(_, x)| x.parent.as_ref().is_some_and(|p| parents.contains(p)))
                .map(|(id, _)| id.clone())
                .collect();
            select(e, kids)
        }
        "select.parent" => match target
            .entity(&e.state)
            .and_then(|id| s.resolved.get(id))
            .and_then(|x| x.parent.clone())
        {
            Some(parent) => select(e, vec![parent]),
            None => Err("The selection has no parent".into()),
        },
        "edit.hide"
        | "edit.unhide_selected"
        | "edit.hide_descendants"
        | "edit.unhide_descendants" => {
            let roots = target.entities(&e.state);
            let branches = id.ends_with("_descendants");
            let hide = match id {
                "edit.hide" => !roots.first().is_some_and(|r| e.state.hidden.contains(r)),
                "edit.unhide_selected" | "edit.unhide_descendants" => false,
                _ => true,
            };
            let entities = if branches {
                with_descendants(&s.resolved, &roots)
            } else {
                roots
            };
            entities.iter().try_for_each(|entity| {
                act(
                    e,
                    "editor.visibility",
                    json!({"entity": entity, "enabled": hide}),
                )
            })
        }
        "edit.unhide_all" => {
            s.local.isolated = None;
            e.state.hidden.clone().iter().try_for_each(|entity| {
                act(
                    e,
                    "editor.visibility",
                    json!({"entity": entity, "enabled": false}),
                )
            })
        }
        "edit.lock" | "edit.lock_descendants" | "edit.unlock_descendants" => {
            let roots = target.entities(&e.state);
            let lock = match id {
                "edit.lock" => !roots.first().is_some_and(|r| e.state.locked.contains(r)),
                "edit.lock_descendants" => true,
                _ => false,
            };
            let entities = if id == "edit.lock" {
                roots
            } else {
                with_descendants(&s.resolved, &roots)
            };
            entities.iter().try_for_each(|entity| {
                act(e, "editor.lock", json!({"entity": entity, "enabled": lock}))
            })
        }
        "view.isolate" => match s.local.isolated.take() {
            // Again: show what isolating hid.
            Some(hid) => hid.iter().try_for_each(|entity| {
                act(
                    e,
                    "editor.visibility",
                    json!({"entity": entity, "enabled": false}),
                )
            }),
            None => {
                let keep = with_descendants(&s.resolved, &target.entities(&e.state));
                // The selection's ancestors stay visible too, so it shows in place.
                let mut shown: std::collections::BTreeSet<String> = keep.into_iter().collect();
                for id in shown.clone() {
                    let mut at = s.resolved.get(&id).and_then(|x| x.parent.clone());
                    while let Some(parent) = at {
                        at = s.resolved.get(&parent).and_then(|x| x.parent.clone());
                        shown.insert(parent);
                    }
                }
                let hide: Vec<String> = s
                    .resolved
                    .keys()
                    .filter(|id| !shown.contains(*id) && !e.state.hidden.contains(*id))
                    .cloned()
                    .collect();
                let result = hide.iter().try_for_each(|entity| {
                    act(
                        e,
                        "editor.visibility",
                        json!({"entity": entity, "enabled": true}),
                    )
                });
                s.local.isolated = Some(hide);
                result
            }
        },
        "edit.copy_reference" => match target.entity(&e.state) {
            Some(entity) => {
                s.local.copy = Some(reference(e, entity));
                Ok(())
            }
            None => Err("Select an entity".into()),
        },
        "hierarchy.reveal" => {
            // Open every collapsed ancestor; the selection stays as it is.
            let mut open = Vec::new();
            for id in target.entities(&e.state) {
                let mut at = s.resolved.get(&id).and_then(|x| x.parent.clone());
                while let Some(parent) = at {
                    at = s.resolved.get(&parent).and_then(|x| x.parent.clone());
                    if e.state.collapsed.contains(&parent) {
                        open.push(parent);
                    }
                }
            }
            open.iter().try_for_each(|entity| {
                act(
                    e,
                    "editor.collapse",
                    json!({"entity": entity, "enabled": false}),
                )
            })
        }
        "prefab.open" => {
            let instance = target
                .entity(&e.state)
                .and_then(|entity| crate::prefab::instance(e.host.store(), &e.state.doc, entity));
            match instance {
                Some(found) => act(e, "editor.scene.open", json!({"scene": found.prefab_doc})),
                None => Err("Not a prefab instance".into()),
            }
        }
        "prefab.revert_all" | "prefab.unpack" => match target.entity(&e.state) {
            Some(entity) => {
                let (cmd, why) = if id == "prefab.unpack" {
                    ("editor.prefab.unpack", "Unpack prefab")
                } else {
                    ("prefab.override.revert", "Revert overrides")
                };
                apply(
                    e,
                    vec![json!({"cmd": cmd, "args": {"entity": entity}})],
                    why,
                )
            }
            None => Err("Select a prefab instance".into()),
        },
        "prefab.apply_all" => match target.entity(&e.state).map(str::to_owned) {
            Some(entity) => crate::prefab::apply(e, &entity, None, None)
                .map(|source| super::note(s, format!("Applied to {source}"))),
            None => Err("Select a prefab instance".into()),
        },
        // Apply To: a source from `prefab::source_choices` (its `@path#id`), or the
        // field-aware choice `{"entity", "path", "destination"}`.
        "prefab.apply_to" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Choice {
                entity: String,
                path: Option<String>,
                destination: String,
            }
            let choice = match target {
                Target::Text(text) if text.starts_with('@') => e
                    .state
                    .selection
                    .first()
                    .map(|entity| Choice {
                        entity: entity.clone(),
                        path: None,
                        destination: text.clone(),
                    })
                    .ok_or_else(|| "Select a prefab instance".into()),
                Target::Text(text) => serde_json::from_str::<Choice>(text).map_err(Into::into),
                _ => Err("Choose a prefab source to apply to".into()),
            };
            choice
                .and_then(|c| {
                    crate::prefab::apply(e, &c.entity, c.path.as_deref(), Some(&c.destination))
                })
                .map(|source| super::note(s, format!("Applied to {source}")))
        }
        "prefab.apply_override" | "prefab.revert_override" => {
            match (target, e.state.selection.first().cloned()) {
                (Target::Field(path), Some(entity)) if id == "prefab.apply_override" => {
                    crate::prefab::apply(e, &entity, Some(path), None)
                        .map(|source| super::note(s, format!("Applied {path} to {source}")))
                }
                (Target::Field(path), Some(entity)) => apply(
                    e,
                    vec![
                        json!({"cmd": "prefab.override.revert", "args": {"entity": entity, "path": path}}),
                    ],
                    "Revert override",
                ),
                (Target::Field(_), None) => Err("Select a prefab instance".into()),
                _ => Err("Right-click an overridden field".into()),
            }
        }
        "create.prefab" | "assets.create.prefab" => match target.entity(&e.state) {
            Some(root) => {
                let root = root.to_owned();
                let files = s.assets.as_ref().map_or(&[][..], |b| b.files.as_slice());
                crate::prefab::from_selection(e, files, &root)
                    .map(|path| super::note(s, format!("Created {path}")))
            }
            None => Err("Select an entity to make a prefab of".into()),
        },
        // View: modes, effects, quality, what draws, gizmos, the camera, the UI.
        id if id.starts_with("view.mode.") => {
            let mode = match id.trim_start_matches("view.mode.") {
                "unlit" | "base_color" => "baseColor".to_owned(),
                "gi_debug" => "gi_diffuse".to_owned(),
                snake => camel(snake),
            };
            view(e, json!({"viewMode": mode}))
        }
        id if id.starts_with("view.debug.") => {
            type Flip = fn(&mut ara_render::RenderDebug);
            let flip: Option<Flip> = match id.trim_start_matches("view.debug.") {
                "ambient_occlusion" => Some(|d| d.ambient_occlusion = !d.ambient_occlusion),
                "shadows" => Some(|d| d.shadows = !d.shadows),
                "soft_shadows" => Some(|d| d.soft_shadows = !d.soft_shadows),
                "screen_reflections" => Some(|d| d.screen_reflections = !d.screen_reflections),
                "reflection_captures" => Some(|d| d.reflection_captures = !d.reflection_captures),
                "global_illumination" => Some(|d| d.global_illumination = !d.global_illumination),
                "screen_gi" => Some(|d| d.screen_gi = !d.screen_gi),
                "volumetric_fog" => Some(|d| d.volumetric_fog = !d.volumetric_fog),
                "temporal_jitter" => Some(|d| d.temporal_jitter = !d.temporal_jitter),
                "full_resolution" => Some(|d| d.full_resolution = !d.full_resolution),
                "reflections" => Some(|d| {
                    use ara_render::ReflectionView as R;
                    d.reflections = if d.reflections == R::Reflections {
                        R::Lit
                    } else {
                        R::Reflections
                    }
                }),
                "reflection_sources" => Some(|d| {
                    use ara_render::ReflectionView as R;
                    d.reflections = if d.reflections == R::Sources {
                        R::Lit
                    } else {
                        R::Sources
                    }
                }),
                "reset" => Some(|d| *d = ara_render::RenderDebug::default()),
                _ => None,
            };
            match flip {
                Some(flip) => {
                    super::scene_overlays::flip_effect(s, flip);
                    Ok(())
                }
                None => Err(format!("{id}: unknown effect").into()),
            }
        }
        id if id.starts_with("view.quality.") => set(
            e,
            "viewport.quality",
            json!(id.trim_start_matches("view.quality.")),
        ),
        "view.fidelity" => {
            use ara_render_fidelity::presets::QualityPreset;
            let low = |c| e.state.resolve_quality(c) == Some(QualityPreset::Low);
            let next = if e.state.quality {
                "low"
            } else if low(e.state.project_quality()) {
                "auto"
            } else {
                "project"
            };
            set(e, "viewport.quality", json!(next))
        }
        "view.show.grid" | "view.show.outline" => {
            flip_setting(e, &format!("viewport.{}", &id[10..]), true)
        }
        "view.show.bounds" | "view.show.colliders" | "view.show.stats" => {
            flip_setting(e, &format!("viewport.{}", &id[10..]), false)
        }
        "view.show.gizmos" | "view.gizmos.toggle" => flip_setting(e, "viewport.gizmos", true),
        "view.gizmos.icons" => flip_setting(e, "viewport.icons", true),
        "view.gizmos.icons3d" => flip_setting(e, "viewport.icons3d", true),
        "view.gizmos.icon_size.small" => set(e, "viewport.iconSize", json!(60)),
        "view.gizmos.icon_size.medium" => set(e, "viewport.iconSize", json!(100)),
        "view.gizmos.icon_size.large" => set(e, "viewport.iconSize", json!(140)),
        "view.gizmos.icon_size.huge" => set(e, "viewport.iconSize", json!(200)),
        "view.gizmos.all_types" => set(e, "viewport.gizmoTypes", json!({})),
        "view.show.orientation" => {
            super::scene_overlays::update(s, "orientation", |d| d.hidden = !d.hidden);
            Ok(())
        }
        "view.overlays.reset" => {
            super::scene_overlays::reset(s);
            Ok(())
        }
        "view.hide_overlays" => {
            let hidden = e.state.hide_overlays;
            view(e, json!({"hideOverlays": !hidden}))
        }
        "view.game" => {
            let on = e.state.game_view;
            view(e, json!({"gameView": !on}))
        }
        "view.camera.front" | "view.camera.back" | "view.camera.left" | "view.camera.right"
        | "view.camera.top" | "view.camera.bottom" => {
            use std::f32::consts::{FRAC_PI_2, PI};
            let (yaw, pitch) = match &id[12..] {
                "front" => (0.0, 0.0),
                "back" => (PI, 0.0),
                "left" => (-FRAC_PI_2, 0.0),
                "right" => (FRAC_PI_2, 0.0),
                "top" => (0.0, 1.54),
                _ => (0.0, -1.54),
            };
            let c = e.state.camera;
            act(
                e,
                "editor.camera",
                json!({"target": c.target.to_array(), "yaw": yaw, "pitch": pitch, "distance": c.distance}),
            )
        }
        "view.camera.align" => match target.entity(&e.state) {
            Some(entity) => act(e, "editor.camera", json!({"entity": entity})),
            None => Err("Select a Camera entity".into()),
        },
        "view.camera.speed.slow" => set(e, "viewport.cameraSpeed", json!(1.0)),
        "view.camera.speed.normal" => set(e, "viewport.cameraSpeed", json!(4.0)),
        "view.camera.speed.fast" => set(e, "viewport.cameraSpeed", json!(16.0)),
        "view.camera.speed_up" | "view.camera.speed_down" => {
            let v = e
                .state
                .settings
                .get("viewport.cameraSpeed")
                .and_then(Value::as_f64)
                .unwrap_or(4.0);
            let v = if id.ends_with("up") { v * 2.0 } else { v / 2.0 };
            set(e, "viewport.cameraSpeed", json!(v.clamp(0.5, 50.0)))
        }
        "view.camera.bookmark.save" => act(e, "editor.camera.bookmark", json!({"action": "save"})),
        "view.camera.fov" | "view.camera.bookmarks" | "view.camera.bookmark.delete" => {
            s.local.overlay_settings = Some("camera");
            Ok(())
        }
        "view.zoom_in" | "view.zoom_out" | "view.zoom_reset" => {
            let v = e
                .state
                .settings
                .get("appearance.uiScale")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let next = match id {
                "view.zoom_in" => (((v + 0.1) * 20.0).round() / 20.0).clamp(0.75, 2.0),
                "view.zoom_out" => (((v - 0.1) * 20.0).round() / 20.0).clamp(0.75, 2.0),
                _ => 1.0,
            };
            set(e, "appearance.uiScale", json!(next))
        }
        "view.density.compact" => set(e, "appearance.density", json!("compact")),
        "view.density.comfortable" => set(e, "appearance.density", json!("comfortable")),
        // Snapping: the overlay's panel holds the steps.
        "snap.grid" | "snap.rotation" | "snap.scale" | "snap.settings" => {
            s.local.overlay_settings = Some("snapping");
            Ok(())
        }
        "snap.surface" => flip_setting(e, "viewport.surfaceSnap", false),
        "snap.vertex" => flip_setting(e, "viewport.vertexSnap", false),
        // Selection.
        "select.siblings" | "select.same_type" | "select.same_asset" => {
            let this = target
                .entity(&e.state)
                .and_then(|entity| s.resolved.get(entity));
            let Some(this) = this else {
                e.report_failure(&*anyhow::anyhow!("Select an entity"));
                return;
            };
            let components = |r: &ara_doc::Record| {
                r.keys()
                    .filter(|k| ara_doc::is_component_key(k))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            let model = |r: &ara_doc::Record| {
                r.get("MeshRenderer")
                    .map(|m| (m.get("model").cloned(), m.get("shape").cloned()))
            };
            let ids = s
                .resolved
                .iter()
                .filter(|(_, x)| match id {
                    "select.siblings" => x.parent == this.parent,
                    "select.same_type" => components(&x.record) == components(&this.record),
                    _ => model(&this.record).is_some() && model(&x.record) == model(&this.record),
                })
                .map(|(id, _)| id.clone())
                .collect();
            select(e, ids)
        }
        "edit.invert_selection" => {
            let ids = s
                .resolved
                .keys()
                .filter(|id| !e.state.selection.contains(id))
                .cloned()
                .collect();
            select(e, ids)
        }
        // Transform.
        "transform.reset_position" | "transform.reset_rotation" | "transform.reset_scale" => {
            let field = id.trim_start_matches("transform.reset_");
            let commands = target
                .entities(&e.state)
                .iter()
                .map(|entity| json!({"cmd": "editor.property.reset", "args": {"entity": entity, "path": format!("Transform.{field}")}}))
                .collect();
            apply(e, commands, &format!("Reset {field}"))
        }
        "transform.move_to_view" => {
            let p = e.state.camera.target;
            let commands = crate::inspector::edit_commands(
                &target.entities(&e.state),
                "Transform.position",
                &json!([p.x, p.y, p.z]),
            );
            apply(e, commands, "Move to view")
        }
        // Window.
        "window.maximize" => {
            let panel = match target {
                Target::Panel(panel) => panel.clone(),
                _ => e.state.focus.clone(),
            };
            let next = if e.state.maximized.as_deref() == Some(panel.as_str()) {
                String::new()
            } else {
                panel
            };
            view(e, json!({"maximized": next}))
        }
        "window.close_panel" => {
            let panel = match target {
                Target::Panel(panel) => panel.clone(),
                _ => e.state.focus.clone(),
            };
            act(e, "editor.layout.close", json!({"panel": panel}))
        }
        // Edit: the entity clipboard, finding, history.
        "edit.copy" | "edit.cut" => {
            let ids = target.entities(&e.state);
            let records: Vec<Value> = ids
                .iter()
                .filter_map(|id| s.resolved.get(id))
                .map(|x| {
                    let mut record = x.record.clone();
                    record.retain(|k, _| ara_doc::is_component_key(k) || k == "name");
                    Value::Object(record.into_iter().collect())
                })
                .collect();
            if records.is_empty() {
                Err("Select an entity to copy".into())
            } else {
                s.local.copy = Some(ara_json::to_canonical_string(
                    &json!({"ara.entities": records}),
                ));
                s.local.entity_clipboard = Some(records);
                if id == "edit.cut" {
                    apply(
                        e,
                        vec![json!({"cmd": "entity.delete", "args": {"entities": ids}})],
                        "Cut entities",
                    )
                } else {
                    Ok(())
                }
            }
        }
        "edit.paste" | "edit.paste_child" => match s.local.entity_clipboard.clone() {
            Some(records) => {
                let parent = (id == "edit.paste_child")
                    .then(|| target.entity(&e.state).map(str::to_owned))
                    .flatten();
                let commands = records
                    .iter()
                    .map(|r| {
                        let mut components = r.clone();
                        let name = components
                            .as_object_mut()
                            .and_then(|m| m.remove("name"))
                            .unwrap_or_else(|| json!("Entity"));
                        let mut args = json!({"name": name, "components": components});
                        if let Some(parent) = &parent {
                            args["parent"] = json!(parent);
                        }
                        json!({"cmd": "entity.create", "args": args})
                    })
                    .collect();
                match e.apply(commands, "Paste entities", None) {
                    Ok(result) => {
                        let ids: Vec<String> = result["outputs"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|o| o["id"].as_str().map(str::to_owned))
                            .collect();
                        select(e, ids)
                    }
                    Err(error) => Err(error.into()),
                }
            }
            None => Err("Copy an entity first".into()),
        },
        "edit.find_scene" => {
            s.local.focus = Some("hierarchy/search");
            Ok(())
        }
        "edit.find_project" => act(e, "editor.layout.show", json!({"panel": "assets"})).map(|()| {
            s.local.focus = Some("assets/search");
        }),
        "edit.goto" => {
            s.local.palette = Some((String::new(), 0));
            s.local.focus = Some("palette/query");
            Ok(())
        }
        "edit.history" => act(e, "editor.layout.show", json!({"panel": "history"})),
        "edit.shortcuts" | "help.shortcuts" => {
            super::preferences::open(s, crate::settings::Scope::User);
            if let Some(view) = s.local.settings.as_mut() {
                view.category = Some("Keyboard shortcuts");
            }
            Ok(())
        }
        // Inspector fields (their right-click menu).
        "field.copy" | "field.copy_path" | "field.paste" | "field.reset" => {
            let Target::Field(path) = target else {
                e.report_failure(&*anyhow::anyhow!("Choose a field"));
                return;
            };
            let path = path.clone();
            let first = s.inspected().first().cloned();
            let e = &mut s.editor;
            match id {
                "field.copy" => {
                    let value = first.and_then(|entity| {
                        let (component, field) = path.split_once('.')?;
                        s.resolved
                            .get(&entity)?
                            .record
                            .get(component)?
                            .get(field)
                            .cloned()
                    });
                    s.local.copy = value.as_ref().map(ara_json::to_canonical_string);
                    s.local.field_clipboard = value;
                    Ok(())
                }
                "field.copy_path" => {
                    s.local.copy = first.map(|entity| format!("{}.{path}", reference(e, &entity)));
                    Ok(())
                }
                "field.paste" => match s.local.field_clipboard.clone() {
                    Some(value) => {
                        let commands =
                            crate::inspector::edit_commands(s.inspected(), &path, &value);
                        apply(&mut s.editor, commands, "Paste value")
                    }
                    None => Err("Copy a value first".into()),
                },
                _ => {
                    let commands = s
                        .inspected()
                        .iter()
                        .map(|entity| json!({"cmd": "editor.property.reset", "args": {"entity": entity, "path": path}}))
                        .collect();
                    apply(&mut s.editor, commands, "Reset value")
                }
            }
        }
        // Components (their ⋯ menu).
        "component.reset" | "component.copy" | "component.paste" | "component.remove" => {
            let Target::Component(name) = target else {
                e.report_failure(&*anyhow::anyhow!("Choose a component"));
                return;
            };
            let name = name.clone();
            match id {
                "component.reset" => super::inspector::component_values(s, &name, None),
                "component.copy" => {
                    let value = s
                        .inspected()
                        .first()
                        .and_then(|entity| s.resolved.get(entity))
                        .and_then(|x| x.record.get(&name).cloned());
                    s.local.component_clipboard = value.map(|v| (name.clone(), v));
                }
                "component.paste" => {
                    if let Some((kind, value)) = s.local.component_clipboard.clone()
                        && kind == name
                    {
                        super::inspector::component_values(s, &name, Some(&value));
                    }
                }
                _ => {
                    let commands = s
                        .inspected()
                        .iter()
                        .map(|entity| json!({"cmd": "component.remove", "args": {"entity": entity, "component": name}}))
                        .collect();
                    s.apply(commands, &format!("Remove {name}"));
                }
            }
            Ok(())
        }
        // Hierarchy filters: Unity's search syntax in the Hierarchy's field.
        id if id.starts_with("hierarchy.filter.")
            && !matches!(
                id,
                "hierarchy.filter.save" | "hierarchy.filter.apply" | "hierarchy.filter.remove"
            ) =>
        {
            s.local.hierarchy_query = match id.trim_start_matches("hierarchy.filter.") {
                "lights" => "t:light",
                "cameras" => "t:camera",
                "meshes" => "t:meshrenderer",
                "audio" => "t:audio",
                "effects" => "t:vfx",
                "prefabs" => "is:prefab",
                _ => "",
            }
            .to_owned();
            Ok(())
        }
        // Help.
        "help.docs" => crate::shell::ops::open_docs(e),
        "help.logs" => crate::shell::ops::open_external(&e.host.root().join(".ara")),
        "console.clear" => {
            s.logs.clear();
            s.local.console_open = None;
            Ok(())
        }
        "console.collapse" => {
            s.local.console_collapse = !s.local.console_collapse;
            Ok(())
        }
        "console.clear_on_play" | "console.error_pause" => {
            let key = if id == "console.clear_on_play" {
                "console.clearOnPlay"
            } else {
                "console.errorPause"
            };
            flip_setting(e, key, false)
        }
        "console.copy" => match target {
            Target::Text(text) => {
                s.local.copy = Some(text.clone());
                Ok(())
            }
            _ => Err("Choose a message".into()),
        },
        // The Material graph page (its canvas keys run these too).
        "graph.add" => {
            super::graph::add(s);
            Ok(())
        }
        "graph.frame" => {
            super::graph::frame(s);
            Ok(())
        }
        "graph.delete" => {
            super::graph::delete_selection(s);
            Ok(())
        }
        "graph.comment" | "graph.group" => {
            super::graph::comment_selection(s);
            Ok(())
        }
        "graph.select_all" => {
            super::graph::select_all(s);
            Ok(())
        }
        // The VFX page's picked module.
        id if id.starts_with("vfx.module.") => {
            let op = id.trim_start_matches("vfx.module.");
            match (
                super::vfx::document(s),
                s.editor.state.resource_item.clone(),
            ) {
                (Some(doc), Some(item)) => {
                    super::vfx::edit(s, &doc, &item, op);
                    Ok(())
                }
                _ => Err("Pick a module in the VFX page first".into()),
            }
        }
        "build.current" => {
            super::build_page::build(s);
            Ok(())
        }
        "build.run" => {
            super::build_page::run(s);
            Ok(())
        }
        "build.open_folder" => {
            super::build_page::open_folder(s);
            Ok(())
        }
        "build.profile.create" => {
            super::build_page::create_profile(s);
            Ok(())
        }
        "build.profiles" | "build.package" => {
            run(s, "window.build", &Target::None);
            Ok(())
        }
        "play.mode.local" | "play.mode.host" | "play.mode.dedicated" => {
            let mode = id.trim_start_matches("play.mode.");
            set(e, "play.mode", json!(mode))
        }
        "play.clients.more" | "play.clients.fewer" => {
            let n = e
                .state
                .settings
                .get("play.clients")
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let n = if id.ends_with("more") {
                n + 1.0
            } else {
                n - 1.0
            };
            set(e, "play.clients", json!(n.clamp(1.0, 8.0)))
        }
        "play.start_paused" => {
            let on = e
                .state
                .settings
                .get("play.startPaused")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            set(e, "play.startPaused", json!(!on))
        }
        "play.restart" => play(e, "play.restart", json!({"background": true})),
        "play.review" => act(e, "editor.focus", json!({"panel": "history"})).and_then(|()| {
            act(
                e,
                "editor.search",
                json!({"panel": "history", "query": "Keep"}),
            )
        }),
        // Play's runtime controls (`crate::play_controls`): Simulate, Eject and Possess,
        // reading and editing running values, and keeping them.
        "play.simulate" => s.play.simulate(e),
        "play.eject" => s.play.control(e, None, false),
        "play.possess" => match target.entity(&e.state).map(str::to_owned) {
            Some(entity) => s.play.control(e, Some(entity), false),
            None => Err("Select an entity the player can control".into()),
        },
        "play.inspect" => s.play.inspect(e),
        "play.runtime.patch" => s.play.apply(e),
        "play.runtime.discard" => s.play.discard(e),
        "play.keep" | "play.keep_driven" => s.play.keep_changes(e, id == "play.keep_driven"),
        "play.keep.apply" => s.play.keep_apply(e),
        "play.keep.cancel" => s.play.keep_cancel(e),
        "play.keep.refresh" => s.play.keep_refresh(e),
        "play.options" | "play.network_profile" => {
            s.local.play_menu = true;
            Ok(())
        }
        "play.settings" => {
            super::preferences::open(s, crate::settings::Scope::Project);
            if let Some(view) = s.local.settings.as_mut() {
                view.category = Some("Networking & Play");
            }
            Ok(())
        }
        "file.new_scene" => {
            super::asset_create::create(s, "scene");
            Ok(())
        }
        "file.open" => {
            s.local.palette = Some(("#".into(), 0));
            s.local.focus = Some("palette/query");
            Ok(())
        }
        "file.close_document" => {
            let panel = match target {
                Target::Panel(panel) => panel.clone(),
                _ => e.state.focus.clone(),
            };
            act(e, "editor.layout.close", json!({"panel": panel}))
        }
        "file.exit" => {
            s.local.quit = true;
            Ok(())
        }
        "file.new_project" | "file.open_project" => {
            let new = id == "file.new_project";
            crate::hub_launch::open(e.host.root(), new, false).and_then(|request| {
                match request.error {
                    Some(error) => Err(error.into()),
                    None => Ok(()),
                }
            })
        }
        "assets.show" => match asset_of(e, target) {
            Some(path) => {
                let folder = path
                    .trim_end_matches('/')
                    .rsplit_once('/')
                    .map_or("", |(d, _)| d)
                    .to_owned();
                act(
                    e,
                    "editor.workspace",
                    json!({"folder": folder, "asset": path}),
                )
                .and_then(|()| act(e, "editor.layout.show", json!({"panel": "assets"})))
            }
            None => Err("The selection has no asset".into()),
        },
        "assets.favourite.toggle" => {
            let path = match target {
                Target::Asset(p) | Target::Folder(p) => p.clone(),
                _ => e
                    .state
                    .asset_path
                    .clone()
                    .unwrap_or_else(|| e.state.asset_folder.clone()),
            };
            let folder = matches!(target, Target::Folder(_))
                || path.ends_with('/')
                || e.state.asset_path.is_none();
            act(
                e,
                "editor.workspace",
                json!({"favourite": path, "favouriteFolder": folder}),
            )
        }
        "assets.validate" => act(e, "editor.validate", json!({}))
            .and_then(|()| act(e, "editor.focus", json!({"panel": "problems"}))),
        "assets.search.scope" => {
            let only = e.state.asset_search_folder_only;
            act(e, "editor.workspace", json!({"folderOnly": !only}))
        }
        "view.camera.default" => act(e, "editor.frame", json!({"all": true})),
        // The OS window, the dock and saved layouts.
        "window.minimize" | "window.maximize_window" => {
            s.local.window = Some(match id {
                "window.minimize" => super::state::WindowRequest::Minimize,
                _ => super::state::WindowRequest::Maximize,
            });
            Ok(())
        }
        "window.show_panel" => match target {
            Target::Panel(panel) => act(e, "editor.focus", json!({"panel": panel})),
            _ => Err("Choose a panel".into()),
        },
        "window.next_document" => act(e, "editor.layout.next", json!({})),
        id if id.starts_with("window.move.") => {
            let slot = match id.trim_start_matches("window.move.") {
                "center" => "view",
                slot => slot,
            };
            let panel = match target {
                Target::Panel(panel) => panel.clone(),
                _ => e.state.focus.clone(),
            };
            act(
                e,
                "editor.layout.move",
                json!({"panel": panel, "slot": slot}),
            )
        }
        "layout.load" => match target {
            Target::Text(name) => act(e, "editor.layout.apply", json!({"name": name})),
            _ => Err("Choose a layout".into()),
        },
        // Scenes, entities and fields named by agents and the palette.
        "file.open_scene" => match target {
            Target::Doc(doc) => act(e, "editor.scene.open", json!({"scene": doc}))
                .and_then(|()| act(e, "editor.focus", json!({"panel": "scene"}))),
            _ => Err("Choose a scene".into()),
        },
        "edit.select_entity" => match target {
            Target::Entity(entity) => {
                select(e, vec![entity.clone()]).and_then(|()| act(e, "editor.frame", json!({})))
            }
            _ => Err("Choose an entity".into()),
        },
        // `Path=json`, or `Path#/pointer=json` inside a compound field.
        "field.set" => {
            let parsed = match target {
                Target::Text(text) => text
                    .split_once('=')
                    .ok_or("Expected Path=value")
                    .map_err(Into::into)
                    .and_then(|(path, value)| {
                        Ok((path.to_owned(), serde_json::from_str::<Value>(value)?))
                    }),
                _ => Err("Expected Path=value".into()),
            };
            parsed.map(|(path, value)| {
                let field = path.split('#').next().unwrap_or(&path).to_owned();
                if path.contains('#') {
                    super::compound::commit_target(s, &path, value, &format!("Set {field}"));
                } else {
                    let commands =
                        crate::inspector::edit_commands(&s.editor.state.selection, &path, &value);
                    s.apply(commands, &format!("Set {field}"));
                }
            })
        }
        "snap.set" => match target {
            Target::Text(text) => text
                .split_once(':')
                .ok_or("Expected kind:value, as move:0.25")
                .map_err(Into::into)
                .and_then(|(kind, value)| {
                    let value: f32 = value.parse()?;
                    let key = match kind {
                        "move" => "moveSnap",
                        "rotate" => "rotateSnap",
                        _ => "scaleSnap",
                    };
                    let mut params = json!({"tool": e.state.tool, "snapping": true});
                    params[key] = json!(value);
                    act(e, "editor.tool", params)
                }),
            _ => Err("Choose an increment, as move:0.25".into()),
        },
        // The Scene camera, tools and gizmos.
        "view.camera.fov.set" => match target {
            Target::Text(text) => text
                .trim()
                .trim_end_matches('°')
                .trim()
                .parse::<f32>()
                .map_err(|_| "The field of view needs a number of degrees".into())
                .and_then(|degrees| act(e, "editor.camera", json!({"fovYDegrees": degrees}))),
            _ => Err("Choose a field of view".into()),
        },
        "view.camera.bookmark.recall" => match target {
            Target::Text(name) => act(
                e,
                "editor.camera.bookmark",
                json!({"action": "recall", "name": name}),
            ),
            _ => {
                s.local.overlay_settings = Some("camera");
                Ok(())
            }
        },
        "view.tool.select" => view(e, json!({"gizmos": false})),
        "view.gizmos.type" => match target {
            Target::Component(kind) => {
                let mut types = e
                    .state
                    .settings
                    .get("viewport.gizmoTypes")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                if types.remove(kind).is_none_or(|v| v != "off") {
                    types.insert(kind.clone(), json!("off"));
                }
                set(e, "viewport.gizmoTypes", Value::Object(types))
            }
            _ => Err("Choose a gizmo type in the Gizmos menu".into()),
        },
        "view.overlays.collapse" => match target {
            Target::Panel(overlay) => {
                super::scene_overlays::update(s, overlay, |d| d.collapsed = !d.collapsed);
                Ok(())
            }
            _ => Err("Choose a Scene overlay".into()),
        },
        // Panels' own switches.
        "console.errors" | "console.warnings" | "console.info" => {
            let i = match id {
                "console.errors" => 0,
                "console.warnings" => 1,
                _ => 2,
            };
            s.local.console[i] = !s.local.console[i];
            Ok(())
        }
        "problems.errors" | "problems.warnings" => {
            let i = usize::from(id == "problems.warnings");
            s.local.problems[i] = !s.local.problems[i];
            Ok(())
        }
        "problems.explain" => match target {
            Target::Text(seq) => seq.parse::<u64>().map_err(Into::into).map(|seq| {
                if !s.local.problems_open.remove(&seq) {
                    s.local.problems_open.insert(seq);
                }
            }),
            _ => Err("Choose a problem".into()),
        },
        "game.capture" | "game.release" => {
            s.local.game_captured = id == "game.capture";
            Ok(())
        }
        "vfx.restart" => {
            s.vfx.restart();
            Ok(())
        }
        "vfx.step" => {
            e.state.preview_playing = false;
            s.vfx.step();
            Ok(())
        }
        "vfx.rate" => match target {
            Target::Text(rate) => rate
                .trim_end_matches('%')
                .parse()
                .map_err(|_| "Rates are percentages".into())
                .map(|rate| s.vfx.set_rate(rate)),
            _ => Err("Choose a rate".into()),
        },
        "vfx.emitter.collapse" | "vfx.emitter.hide" | "vfx.emitter.isolate" => match target {
            Target::Text(name) => {
                use super::vfx::Emitter;
                let toggle = match id {
                    "vfx.emitter.collapse" => Emitter::Collapse,
                    "vfx.emitter.hide" => Emitter::Hide,
                    _ => Emitter::Isolate,
                };
                s.vfx.emitter(name, toggle);
                Ok(())
            }
            _ => Err("Choose an emitter".into()),
        },
        "tools.cs.reload" => super::scripts::reload(s),
        "tools.cs.solution" => super::scripts::solution(s),
        "component.new_script" => {
            s.local.script_attach = target.entities(&e.state);
            s.local.add_component = None;
            super::asset_create::create(s, "script");
            Ok(())
        }
        "assets.open_ide" => match asset_of(e, target).filter(|p| p.ends_with(".cs")) {
            Some(path) => {
                super::asset_ops::open_in_ide(s, &path, None);
                Ok(())
            }
            None => Err("Select a C# script".into()),
        },
        "component.add" => {
            s.local.add_component = Some(Default::default());
            s.local.focus = Some("inspector/add/search");
            Ok(())
        }
        // Agents: look where one is, follow it, undo what a turn did.
        "agents.view" | "agents.follow" => match target {
            Target::Text(agent) if id == "agents.view" => {
                act(e, "editor.follow", json!({"agent": agent}))
                    .and_then(|()| act(e, "editor.follow", json!({})))
            }
            Target::Text(agent) if e.state.follow.as_ref() == Some(agent) => {
                act(e, "editor.follow", json!({}))
            }
            Target::Text(agent) => act(e, "editor.follow", json!({"agent": agent})),
            _ => Err("Choose an agent".into()),
        },
        "agents.stop_following" => act(e, "editor.follow", json!({})),
        // A turn is one transaction, or several sharing a `gesture-turn:<group>|` label;
        // newest first, so each revert applies on top of what came after it.
        "agents.undo_turn" => match target {
            Target::Text(turn) => {
                let marker = format!("gesture-turn:{turn}|");
                let store = e.host.store();
                let txns: Vec<String> = store
                    .history()
                    .iter()
                    .rev()
                    .filter(|x| {
                        x.id.0 == *turn
                            || x.label.as_deref().is_some_and(|l| l.starts_with(&marker))
                    })
                    .filter(|x| store.reverted_by(&x.id).is_none())
                    .map(|x| x.id.0.clone())
                    .collect();
                txns.iter()
                    .try_for_each(|txn| act(e, "review.revert", json!({"txn": txn})))
            }
            _ => Err("Choose an agent turn".into()),
        },
        "history.copy" => match target {
            Target::Text(txn) => {
                let found = e.host.store().history().iter().find(|x| x.id.0 == *txn);
                match found.map(serde_json::to_value) {
                    Some(Ok(value)) => {
                        s.local.copy = Some(ara_json::to_canonical_string(&value));
                        Ok(())
                    }
                    Some(Err(error)) => Err(error.into()),
                    None => Err("That transaction is not in this project's history".into()),
                }
            }
            _ => Err("Choose a transaction".into()),
        },
        "help.search" => {
            s.local.palette = Some((String::new(), 0));
            Ok(())
        }
        "create.mesh" => act(e, "editor.focus", json!({"panel": "assets"})),
        // Import…: the system's file chooser, into the target folder or the one Assets shows.
        "assets.import" => {
            let folder = match target {
                Target::Folder(path) => Some(path.trim_end_matches('/').to_owned()),
                Target::Asset(path) => {
                    Some(path.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned())
                }
                _ => None,
            };
            let start = e.host.root().to_owned();
            let files = stillkit::dialog::pick_files("Import into the project", Some(&start), true);
            if !files.is_empty() {
                super::asset_ops::import(s, files, folder.as_deref());
            }
            Ok(())
        }
        "assets.import.cancel" => s.importer.cancel(e),
        "assets.show_all" => {
            s.local.assets_view.show_all = !s.local.assets_view.show_all;
            Ok(())
        }
        "assets.rebuild_thumbnails" => {
            s.previews.rebuild();
            Ok(())
        }
        "assets.move" => match asset_of(e, target) {
            Some(path) => {
                s.local.assets_view.moving = Some((path, String::new()));
                s.local.focus = Some("assets/move/search");
                Ok(())
            }
            None => Err("Select an asset or folder to move".into()),
        },
        // Saved searches and favourites: by name (or the query typed, to save it).
        "assets.search.save" => {
            let name = match target {
                Target::Text(name) => name.trim().to_owned(),
                _ => e
                    .state
                    .searches
                    .get("assets")
                    .map(|q| q.trim().to_owned())
                    .unwrap_or_default(),
            };
            match name.is_empty() {
                true => Err("Type a search to save first".into()),
                false => act(e, "editor.workspace", json!({"saveSearch": name})),
            }
        }
        "assets.search.apply" | "assets.search.remove" | "assets.favourite.open" => match target {
            Target::Text(name) | Target::Asset(name) | Target::Folder(name) => {
                let key = match id {
                    "assets.search.apply" => "openSearch",
                    "assets.search.remove" => "removeSearch",
                    _ => "openFavourite",
                };
                act(e, "editor.workspace", json!({key: name}))
            }
            _ => Err("Choose a saved search or favourite".into()),
        },
        "tools.terminal" => crate::shell::ops::open_terminal(e.host.root()),
        "tools.references" => match asset_of(e, target) {
            Some(path) => {
                s.local.inspect_asset = true;
                act(e, "editor.workspace", json!({"asset": path}))
            }
            None => Err("Select an asset to see what it uses and what uses it".into()),
        },
        // Window › a panel (last, so the window.* commands above take theirs first).
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
        _ => Err(format!("{id}: unknown command").into()),
    };
    // Whatever changed the document shows at once, not on the host's next pass.
    if s.editor.host.store().revision() != revision {
        s.resolve();
    }
    if let Err(error) = result {
        s.editor.report_failure(&*error);
    }
}

/// The registry spec of `id`, for titles, icons and shortcuts.
pub(super) fn spec(id: &str) -> Option<&'static crate::catalog::Command> {
    crate::catalog::get(id)
}

/// The studio's keyboard map: the registry's default chords for the commands it runs.
pub(super) fn for_key(key: u16, mods: stillkit::el::Modifiers) -> Option<&'static str> {
    let c = mods.ctrl;
    let sh = mods.shift;
    if mods.alt {
        return (key == 0x74).then_some("play.simulate");
    }
    Some(match (c, sh, key) {
        (true, false, 0x5A) => "edit.undo",
        (true, true, 0x5A) | (true, false, 0x59) => "edit.redo",
        (true, false, 0x53) => "file.save",
        (true, false, 0x43) => "edit.copy",
        (true, false, 0x58) => "edit.cut",
        (true, false, 0x56) => "edit.paste",
        (true, false, 0x46) => "edit.find_scene",
        (true, true, 0x46) => "edit.find_project",
        (true, true, 0x42) => "build.current",
        (true, false, 0x4E) => "file.new_scene",
        (true, false, 0x4F) => "file.open",
        (true, false, 0x57) => "file.close_document",
        (true, false, 0x42) => "assets.show",
        (true, false, 0x44) => "edit.duplicate",
        (true, false, 0x41) => "edit.select_all",
        (true, true, 0x41) => "edit.deselect",
        (true, false, 0x4B) | (true, true, 0x50) => "edit.palette",
        (true, false, 0x52) => "tools.cs.reload",
        (true, true, 0x4E) => "create.empty",
        (false, true, 0x41) => "create.at_cursor",
        (true, false, 0xBC) => "edit.preferences",
        (false, false, 0x2E) | (false, false, 0x08) => "edit.delete",
        (false, false, 0x48) => "edit.hide",
        (false, true, 0x48) => "view.isolate",
        (false, true, 0x47) => "view.hide_overlays",
        (true, true, 0x47) => "view.gizmos.toggle",
        (false, false, 0x61) => "view.camera.front",
        (true, false, 0x61) => "view.camera.back",
        (false, false, 0x63) => "view.camera.right",
        (true, false, 0x63) => "view.camera.left",
        (false, false, 0x67) => "view.camera.top",
        (true, false, 0x67) => "view.camera.bottom",
        (false, true, 0x20) => "window.maximize",
        (false, false, 0x46) => "view.frame",
        (false, false, 0x24) => "view.frame_all",
        (false, false, 0x57) => "view.tool.move",
        (false, false, 0x45) => "view.tool.rotate",
        (false, false, 0x52) => "view.tool.scale",
        (false, false, 0x54) => "view.tool.universal",
        (false, false, 0x51) => "view.tool.select",
        (false, false, 0x70) => "help.search",
        (true, false, 0x09) => "window.next_document",
        (false, false, 0x74) => "play.start",
        (false, false, 0x75) => "play.pause",
        (false, false, 0x76) => "play.step",
        (false, true, 0x74) => "play.stop",
        (false, false, 0x77) => "play.eject",
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
