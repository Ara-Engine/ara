//! The registry table. Commands that are not built yet stay registered with the
//! reason, so menus explain what is missing instead of hiding it.
use super::{Avail, Ctx, Run, RunFn, Spec, Target};
use crate::{Editor, shell::Shell};
use serde_json::{Value, json};
use std::sync::LazyLock;

pub(super) static COMMANDS: LazyLock<Vec<Spec>> = LazyLock::new(build);

fn cmd(
    id: &'static str,
    title: &'static str,
    icon: &'static str,
    keys: &'static str,
    summary: &'static str,
    run: RunFn,
) -> Spec {
    Spec {
        id,
        title,
        icon,
        keys,
        summary,
        check: ready,
        label: None,
        run: Run::Do(run),
    }
}
fn later(
    id: &'static str,
    title: &'static str,
    icon: &'static str,
    keys: &'static str,
    reason: &'static str,
) -> Spec {
    Spec {
        id,
        title,
        icon,
        keys,
        summary: reason,
        check: ready,
        label: None,
        run: Run::Later(reason),
    }
}
impl Spec {
    fn when(mut self, check: fn(&Ctx) -> Avail) -> Self {
        self.check = check;
        self
    }
    fn labelled(mut self, label: fn(&Ctx) -> String) -> Self {
        self.label = Some(label);
        self
    }
}

type R = ara_ui::Result<()>;
fn ready(_: &Ctx) -> Avail {
    Avail::Ready
}
fn has_entity(c: &Ctx) -> Avail {
    if c.target.entity(c.state).is_some() {
        Avail::Ready
    } else {
        Avail::Blocked("Select an entity first".into())
    }
}
fn has_asset(c: &Ctx) -> Avail {
    match (c.target, &c.state.asset_path) {
        (Target::Asset(_) | Target::Folder(_), _) | (_, Some(_)) => Avail::Ready,
        _ => Avail::Blocked("Select an asset in the Assets panel first".into()),
    }
}
fn scene_doc(c: &Ctx) -> Avail {
    if c.store.workspace().doc(&c.state.doc).is_some() {
        Avail::Ready
    } else {
        Avail::Blocked("Open a scene first".into())
    }
}
fn graph_open(c: &Ctx) -> Avail {
    if matches!(c.state.focus.as_str(), "material" | "vfx" | "animation")
        && c.state.resource.is_some()
    {
        Avail::Ready
    } else {
        Avail::Blocked("Open a material, VFX or animation graph first".into())
    }
}
fn setting(c: &Ctx, key: &str) -> Value {
    c.state.settings.get(key).cloned().unwrap_or(Value::Null)
}
fn is(c: &Ctx, key: &str, value: &str) -> Avail {
    Avail::Checked(setting(c, key).as_str() == Some(value))
}
fn mode(c: &Ctx, mode: &str) -> Avail {
    Avail::Checked(c.state.view_mode == mode)
}
fn flag(c: &Ctx, key: &str) -> Avail {
    Avail::Checked(setting(c, key).as_bool().unwrap_or(false))
}
fn with_components(c: &Ctx, names: &[&str]) -> Avail {
    match names.iter().find(|n| c.store.schema().get(n).is_none()) {
        Some(missing) => Avail::Blocked(format!(
            "`{missing}` is not registered in this project's schema"
        )),
        None => scene_doc(c),
    }
}

fn act(e: &mut Editor, method: &str, params: Value) -> R {
    e.action(method, params)?;
    Ok(())
}
fn apply(e: &mut Editor, commands: Vec<Value>, why: &str) -> R {
    if !commands.is_empty() {
        e.apply(commands, why, None)?;
    }
    Ok(())
}
fn focus(e: &mut Editor, panel: &str) -> R {
    act(e, "editor.focus", json!({"panel": panel}))
}
fn set(e: &mut Editor, key: &str, value: Value) -> R {
    act(
        e,
        "editor.settings.set",
        json!({"key": key, "value": value}),
    )
}
fn toggle(e: &mut Editor, key: &str) -> R {
    let current = e
        .state
        .settings
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    set(e, key, json!(!current))
}
fn view(e: &mut Editor, params: Value) -> R {
    act(e, "editor.view", params)
}
fn camera(e: &mut Editor, yaw: f32, pitch: f32) -> R {
    let c = e.state.camera;
    act(
        e,
        "editor.camera",
        json!({"target": c.target.to_array(), "yaw": yaw, "pitch": pitch, "distance": c.distance}),
    )
}
fn layout(e: &mut Editor, name: &str) -> R {
    act(e, "editor.layout.apply", json!({"name": name}))
}
fn panel_target(s: &Shell, e: &Editor, t: &Target) -> String {
    match t {
        Target::Panel(p) => p.clone(),
        _ => s.active_panel(e),
    }
}
fn move_panel(s: &mut Shell, e: &mut Editor, t: &Target, slot: &str) -> R {
    let panel = panel_target(s, e, t);
    act(
        e,
        "editor.layout.move",
        json!({"panel": panel, "slot": slot}),
    )
}
/// Create an entity at the view centre (or as a child of the target) and select it.
fn create(e: &mut Editor, t: &Target, name: &str, components: Value, child: bool) -> R {
    let mut args = json!({"name": name, "components": components});
    if args["components"].get("Transform").is_none() {
        let target = e.state.camera.target;
        args["components"]["Transform"] =
            json!({"position": [target.x, target.y.max(0.), target.z]});
    }
    if child {
        let parent = t
            .entity(&e.state)
            .ok_or("select a parent entity")?
            .to_owned();
        args["parent"] = parent.into();
        args["components"]["Transform"] = json!({});
    }
    let result = e.apply(
        vec![json!({"cmd": "entity.create", "args": args})],
        &format!("Create {name}"),
        None,
    )?;
    if let Some(id) = result["outputs"][0]["id"].as_str() {
        act(e, "editor.select", json!({"entities": [id]}))?;
    }
    Ok(())
}
fn shape(e: &mut Editor, t: &Target, name: &str, shape: &str) -> R {
    create(
        e,
        t,
        name,
        json!({"MeshRenderer": {"shape": shape}}),
        matches!(t, Target::Entity(_)),
    )
}
fn reference(e: &Editor, entity: &str) -> String {
    let path = e
        .host
        .store()
        .workspace()
        .path_of(&e.state.doc)
        .unwrap_or("")
        .to_owned();
    format!("@{path}#{entity}")
}
fn resolved(e: &Editor) -> ara_ui::Result<ara_doc::prefab::Resolved> {
    crate::world::resolve(e.host.store(), &e.state.doc).map_err(|d| format!("{d:?}").into())
}
fn select(e: &mut Editor, entities: Vec<String>) -> R {
    act(e, "editor.select", json!({"entities": entities}))
}
fn play(e: &mut Editor, method: &str, params: Value) -> R {
    act(e, method, params)
}
fn play_params(e: &Editor) -> Value {
    let s = &e.state.settings;
    let text = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let mode = text("play.mode");
    let mut params = json!({
        "window": true,
        "mode": mode,
        "tickHz": text("play.tickHz").parse::<u32>().unwrap_or(60),
        "startPaused": s.get("play.startPaused").and_then(Value::as_bool).unwrap_or(false),
        "seed": s.get("play.seed").and_then(Value::as_f64).unwrap_or(0.) as u64,
        "hotReload": s.get("play.hotReload").and_then(Value::as_bool).unwrap_or(true),
    });
    if mode != "local" {
        params["clients"] =
            json!(s.get("play.clients").and_then(Value::as_f64).unwrap_or(1.) as u32);
    }
    params
}
fn undo_label(c: &Ctx, redo: bool) -> String {
    let verb = if redo { "Redo" } else { "Undo" };
    let label = if c.state.focus == "settings" {
        if redo {
            c.state.settings_redo.clone()
        } else {
            c.state.settings_undo.clone()
        }
    } else {
        let author = ara_ops::Author::Human;
        let txn = if redo {
            c.store
                .next_redo(&author)
                .and_then(|u| u.reverts.as_ref())
                .and_then(|id| c.store.txn(id))
        } else {
            c.store.next_undo(&author)
        };
        txn.and_then(|t| t.why.clone().or_else(|| t.label.clone()))
    };
    match label {
        // Long agent reasons are cut so the item's shortcut stays in the menu.
        Some(l) if l.chars().count() > 32 => {
            let short: String = l.chars().take(31).collect();
            format!("{verb} \u{201c}{}\u{2026}\u{201d}", short.trim_end())
        }
        Some(l) => format!("{verb} \u{201c}{l}\u{201d}"),
        None => verb.into(),
    }
}
fn can_undo(c: &Ctx, redo: bool) -> Avail {
    let available = if c.state.focus == "settings" {
        if redo {
            c.state.settings_redo.is_some()
        } else {
            c.state.settings_undo.is_some()
        }
    } else if redo {
        c.store.next_redo(&ara_ops::Author::Human).is_some()
    } else {
        c.store.next_undo(&ara_ops::Author::Human).is_some()
    };
    if available {
        Avail::Ready
    } else {
        Avail::Blocked(format!("Nothing to {}", if redo { "redo" } else { "undo" }))
    }
}
fn panel_open(c: &Ctx, panel: &str) -> Avail {
    Avail::Checked(crate::state::panel_visible(c.state, panel))
}
/// The VFX module a command acts on: the target, else the selected module.
fn vfx_item(e: &Editor, t: &Target) -> Option<String> {
    match t {
        Target::Text(item) if item.split('|').count() == 3 => Some(item.clone()),
        _ => e
            .state
            .resource_item
            .clone()
            .filter(|i| e.state.focus == "vfx" && i.split('|').count() == 3),
    }
}
fn vfx_module_target(c: &Ctx) -> Avail {
    let ok = match c.target {
        Target::Text(item) => item.split('|').count() == 3,
        _ => {
            c.state.focus == "vfx"
                && c.state
                    .resource_item
                    .as_ref()
                    .is_some_and(|i| i.split('|').count() == 3)
        }
    };
    if ok {
        Avail::Ready
    } else {
        Avail::Blocked("Select a VFX module first".into())
    }
}
fn vfx_module(s: &mut Shell, e: &mut Editor, t: &Target, operation: &str) -> R {
    let item = vfx_item(e, t).ok_or("select a VFX module")?;
    s.edit_vfx_module(e, &item, operation)
}
fn transform_reset(e: &mut Editor, t: &Target, field: &str) -> R {
    let commands = t.entities(&e.state).iter().map(|entity| json!({"cmd": "editor.property.reset", "args": {"entity": entity, "path": format!("Transform.{field}")}})).collect();
    apply(e, commands, &format!("Reset {field}"))
}

fn build() -> Vec<Spec> {
    let mut c = Vec::with_capacity(240);
    // File
    c.extend([
        cmd("file.new_scene", "New Scene…", "file", "Ctrl+N", "Create an empty scene under levels/ and open it.", |s, e, _| s.new_scene(e)),
        later("file.new_project", "New Project…", "folder", "", "Create projects with `ara init <dir>`; the in-editor project wizard comes with the launcher."),
        cmd("file.open", "Open…", "search", "Ctrl+O", "Quick-open a scene or asset by name.", |s, e, _| s.open_palette(e, "#")),
        later("file.open_project", "Open Project…", "folder", "", "One editor process owns one project; start `ara-editor <project>` for another."),
        cmd("file.open_scene", "Open Scene", "box", "", "Open a scene document in the Scene view.", |s, e, t| {
            let Target::Doc(doc) = t else { return Err("choose a scene".into()) };
            act(e, "editor.select", json!({"doc": doc}))?;
            s.recent(doc);
            focus(e, "scene")
        }),
        cmd("file.save", "Save", "check", "Ctrl+S", "Write every changed document to disk now.", |s, e, _| {
            e.host.flush();
            s.status_note = Some("All changes saved".into());
            Ok(())
        }),
        later("file.save_as", "Save As…", "file", "Ctrl+Shift+S", "Documents save in place through the host journal; Save As needs a typed document-copy command."),
        cmd("file.save_all", "Save All", "check", "Ctrl+Alt+S", "Write every changed document to disk now.", |s, e, _| {
            e.host.flush();
            s.status_note = Some("All changes saved".into());
            Ok(())
        }),
        later("file.revert", "Revert to Saved", "undo-2", "", "Use Undo or `ara revert`; reloading from disk needs a host revert-to-file command."),
        cmd("assets.import", "Import…", "plus", "Ctrl+Shift+I", "Copy files into the current asset folder and import them.", |s, _, _| {
            s.window_request = Some(super::super::WindowRequest::Import);
            Ok(())
        }),
        later("file.export_selection", "Export Selection…", "file", "", "Selection export lands with ara-export presets in the editor."),
        later("build.profiles", "Build Profiles…", "package", "", "Export presets run from `ara export`; the in-editor Build window is not built yet."),
        later("build.current", "Build Current Profile", "package", "Ctrl+Shift+B", "Run `ara export` for now; the in-editor Build window is not built yet."),
        later("build.package", "Package…", "package", "", "Run `ara pack` for now; packaging from the editor is not built yet."),
        cmd("build.open_folder", "Open Build Folder", "folder", "", "Show the project's build output folder.", |_, e, _| {
            let root = e.host.root().join("build");
            std::fs::create_dir_all(&root)?;
            super::super::open_external(&root)
        }),
        cmd("file.close_document", "Close Document", "x", "Ctrl+W", "Close the active tool document tab.", |s, e, t| {
            let panel = panel_target(s, e, t);
            act(e, "editor.layout.close", json!({"panel": panel}))
        }),
        later("file.close_project", "Close Project", "x", "", "One editor process owns one project; close the window to close it."),
        cmd("file.exit", "Exit", "x", "Alt+F4", "Close the editor; documents are saved first.", |s, e, _| {
            e.host.flush();
            s.window_request = Some(super::super::WindowRequest::Quit);
            Ok(())
        }),
    ]);
    // Edit
    c.extend([
        cmd("edit.undo", "Undo", "undo-2", "Ctrl+Z", "Undo your last change in the focused context.", |_, e, _| {
            if e.state.focus == "settings" { act(e, "editor.settings.undo", json!({})) } else { act(e, "history.undo", json!({})) }
        }).when(|c| can_undo(c, false)).labelled(|c| undo_label(c, false)),
        cmd("edit.redo", "Redo", "redo-2", "Ctrl+Y / Ctrl+Shift+Z", "Redo your last undone change.", |_, e, _| {
            if e.state.focus == "settings" { act(e, "editor.settings.undo", json!({"redo": true})) } else { act(e, "history.redo", json!({})) }
        }).when(|c| can_undo(c, true)).labelled(|c| undo_label(c, true)),
        later("edit.history", "History…", "clock", "Ctrl+H", "The History panel (typed command stack) arrives with the Phase 2 tool rebuilds; `ara history` lists it now."),
        cmd("edit.cut", "Cut", "scissors", "Ctrl+X", "Copy the selected entities, then delete them.", |s, e, t| {
            s.copy_entities(e, t)?;
            let entities = t.entities(&e.state);
            apply(e, vec![json!({"cmd": "entity.delete", "args": {"entities": entities}})], "Cut entities")
        }).when(has_entity),
        cmd("edit.copy", "Copy", "copy", "Ctrl+C", "Copy the selected entities.", |s, e, t| s.copy_entities(e, t)).when(has_entity),
        cmd("edit.paste", "Paste", "clipboard", "Ctrl+V", "Paste copied entities into the scene.", |s, e, _| s.paste_entities(e, None))
            .when(|c| if c.clipboard_entities { Avail::Ready } else { Avail::Blocked("Copy an entity first".into()) }),
        cmd("edit.paste_child", "Paste as Child", "clipboard", "Ctrl+Shift+V", "Paste copied entities under the selected entity.", |s, e, t| {
            let parent = t.entity(&e.state).map(str::to_owned);
            s.paste_entities(e, parent)
        }).when(|c| if !c.clipboard_entities { Avail::Blocked("Copy an entity first".into()) } else { has_entity(c) }),
        cmd("edit.duplicate", "Duplicate", "copy", "Ctrl+D", "Duplicate the selected entity subtree.", |_, e, t| {
            let commands = t.entities(&e.state).iter().map(|entity| json!({"cmd": "editor.entity.duplicate", "args": {"entity": entity}})).collect();
            apply(e, commands, "Duplicate")
        }).when(has_entity),
        cmd("edit.delete", "Delete", "trash", "Del", "Delete the selected entities.", |_, e, t| {
            let entities = t.entities(&e.state);
            apply(e, vec![json!({"cmd": "entity.delete", "args": {"entities": entities}})], "Delete selection")
        }).when(has_entity),
        cmd("edit.rename", "Rename", "pencil", "F2", "Rename the selected entity inline.", |s, e, t| {
            if let Target::Entity(id) = t && !e.state.selection.contains(id) {
                select(e, vec![id.clone()])?;
            }
            s.rename = true;
            Ok(())
        }).when(has_entity),
        cmd("edit.copy_reference", "Copy Reference", "link", "Ctrl+Shift+C", "Copy `@path#id` for the selected entity.", |s, e, t| {
            let entity = t.entity(&e.state).ok_or("select an entity")?.to_owned();
            s.copy_text(reference(e, &entity));
            Ok(())
        }).when(has_entity),
        cmd("edit.select_all", "Select All", "", "Ctrl+A", "Select every entity in the scene.", |_, e, _| {
            let all = resolved(e)?.keys().cloned().collect();
            select(e, all)
        }).when(scene_doc),
        cmd("edit.select_entity", "Select Entity", "box", "", "Select and frame an entity (target entity).", |_, e, t| {
            let Target::Entity(id) = t else { return Err("choose an entity".into()) };
            select(e, vec![id.clone()])?;
            act(e, "editor.frame", json!({}))
        }),
        cmd("edit.deselect", "Deselect", "", "Ctrl+Shift+A", "Clear the selection.", |_, e, _| select(e, Vec::new())),
        cmd("edit.invert_selection", "Invert Selection", "", "Ctrl+I", "Select every entity that is not selected.", |_, e, _| {
            let all = resolved(e)?.keys().filter(|id| !e.state.selection.contains(id)).cloned().collect();
            select(e, all)
        }).when(scene_doc),
        cmd("select.children", "Select Children", "", "", "Select the direct children of the selected entities.", |_, e, t| {
            let parents = t.entities(&e.state);
            let kids = resolved(e)?.iter().filter(|(_, x)| x.parent.as_ref().is_some_and(|p| parents.contains(p))).map(|(id, _)| id.clone()).collect();
            select(e, kids)
        }).when(has_entity),
        cmd("select.parent", "Select Parent", "", "", "Select the parent of the selected entity.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select an entity")?.to_owned();
            let parent = resolved(e)?.get(&entity).and_then(|x| x.parent.clone()).ok_or("the entity is at the scene root")?;
            select(e, vec![parent])
        }).when(has_entity),
        cmd("select.siblings", "Select Siblings", "", "", "Select entities that share the selected entity's parent.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select an entity")?.to_owned();
            let all = resolved(e)?;
            let parent = all.get(&entity).and_then(|x| x.parent.clone());
            select(e, all.iter().filter(|(_, x)| x.parent == parent).map(|(id, _)| id.clone()).collect())
        }).when(has_entity),
        cmd("select.same_type", "Select Same Type", "", "", "Select entities with the same component set.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select an entity")?.to_owned();
            let all = resolved(e)?;
            let kinds = |r: &ara_doc::Record| r.keys().filter(|k| ara_doc::is_component_key(k)).cloned().collect::<Vec<_>>();
            let wanted = all.get(&entity).map(|x| kinds(&x.record)).unwrap_or_default();
            select(e, all.iter().filter(|(_, x)| kinds(&x.record) == wanted).map(|(id, _)| id.clone()).collect())
        }).when(has_entity),
        cmd("select.same_asset", "Select Same Asset", "", "", "Select entities that draw the same model.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select an entity")?.to_owned();
            let all = resolved(e)?;
            let model = |r: &ara_doc::Record| r.get("MeshRenderer").map(|m| (m.get("model").cloned(), m.get("shape").cloned()));
            let wanted = all.get(&entity).and_then(|x| model(&x.record));
            select(e, all.iter().filter(|(_, x)| wanted.is_some() && model(&x.record) == wanted).map(|(id, _)| id.clone()).collect())
        }).when(has_entity),
        later("select.layer", "By Layer…", "", "", "Layers and tags are not in the schema yet."),
        cmd("edit.find_scene", "Find in Scene", "search", "Ctrl+F", "Search the hierarchy.", |s, e, _| s.focus_search(e, "hierarchy")),
        cmd("edit.find_project", "Find in Project", "search", "Ctrl+Shift+F", "Search project assets.", |s, e, _| s.focus_search(e, "assets")),
        cmd("edit.palette", "Command Palette", "command", "Ctrl+K / Ctrl+Shift+P", "Search and run any command.", |s, e, _| s.open_palette(e, "")),
        cmd("edit.goto", "Go to Entity or Asset", "search", "Ctrl+P", "Jump to an entity, scene or asset by name.", |s, e, _| s.open_palette(e, "@")),
        cmd("snap.toggle", "Snap", "magnet", "", "Toggle transform snapping.", |_, e, _| {
            let tool = e.state.tool;
            act(e, "editor.tool", json!({"tool": tool, "snapping": !e.state.snapping}))
        }).when(|c| Avail::Checked(c.state.snapping)),
        cmd("snap.grid", "Grid Snap Size…", "grid-2x2", "", "Choose the move snap increment.", |s, _, _| { s.open_value_menu("move"); Ok(()) }),
        cmd("snap.rotation", "Rotation Snap Size…", "rotate-3d", "", "Choose the rotation snap increment.", |s, _, _| { s.open_value_menu("rotate"); Ok(()) }),
        cmd("snap.scale", "Scale Snap Size…", "scaling", "", "Choose the scale snap increment.", |s, _, _| { s.open_value_menu("scale"); Ok(()) }),
        cmd("snap.set", "Set Snap Increment", "magnet", "", "Set a snap increment: target `move:0.25`, `rotate:15` or `scale:0.1`.", |_, e, t| {
            let Target::Text(text) = t else { return Err("choose an increment".into()) };
            let (kind, value) = text.split_once(':').ok_or("expected kind:value")?;
            let value: f32 = value.parse()?;
            let key = match kind { "move" => "moveSnap", "rotate" => "rotateSnap", _ => "scaleSnap" };
            let mut params = json!({"tool": e.state.tool, "snapping": true});
            params[key] = json!(value);
            act(e, "editor.tool", params)
        }),
        later("snap.surface", "Surface Snap", "magnet", "", "Surface snapping needs scene raycasts from ara-physics queries in the editor."),
        later("snap.vertex", "Vertex Snap (hold V)", "magnet", "", "Vertex snapping needs mesh vertex queries from ara-asset in the editor."),
        cmd("snap.settings", "Snap Settings…", "settings-2", "", "Open snap and viewport preferences.", |s, e, _| s.open_settings(e, "user", "Viewport")),
        cmd("transform.reset_position", "Reset Position", "", "", "Reset position to the default or prefab value.", |_, e, t| transform_reset(e, t, "position")).when(has_entity),
        cmd("transform.reset_rotation", "Reset Rotation", "", "", "Reset rotation to the default or prefab value.", |_, e, t| transform_reset(e, t, "rotation")).when(has_entity),
        cmd("transform.reset_scale", "Reset Scale", "", "", "Reset scale to the default or prefab value.", |_, e, t| transform_reset(e, t, "scale")).when(has_entity),
        later("transform.align_view", "Align to View", "", "", "Aligning rotation to the camera needs a view-matrix to Euler command."),
        cmd("transform.move_to_view", "Move to View", "", "", "Move the selection to the camera's focus point.", |_, e, t| {
            let p = e.state.camera.target;
            let commands = crate::inspector::edit_commands(&t.entities(&e.state), "Transform.position", &json!([p.x, p.y, p.z]));
            apply(e, commands, "Move to view")
        }).when(has_entity),
        later("edit.transform.drop", "Drop to Floor", "", "End", "Dropping to the floor needs physics raycasts in the editor."),
        cmd("edit.preferences", "Preferences…", "settings-2", "Ctrl+,", "Open your editor preferences.", |s, e, _| s.open_settings(e, "user", "")),
        cmd("edit.project_settings", "Project Settings…", "settings-2", "Ctrl+Alt+,", "Open settings shared by the project.", |s, e, _| s.open_settings(e, "project", "")),
        cmd("edit.shortcuts", "Keyboard Shortcuts…", "command", "Ctrl+Alt+K", "Search, rebind and choose keymap presets.", |s, e, _| s.open_settings(e, "user", "Keyboard shortcuts")),
        cmd("edit.hide", "Hide", "eye-off", "H", "Hide the entity in the viewport only.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select an entity")?.to_owned();
            let hidden = e.state.hidden.contains(&entity);
            act(e, "editor.visibility", json!({"entity": entity, "enabled": !hidden}))
        }).when(|c| match c.target.entity(c.state) { Some(id) => Avail::Checked(c.state.hidden.contains(id)), None => has_entity(c) }),
        cmd("edit.unhide_all", "Show All", "eye", "Alt+H", "Show every hidden entity.", |_, e, _| {
            for entity in e.state.hidden.clone() {
                act(e, "editor.visibility", json!({"entity": entity, "enabled": false}))?;
            }
            Ok(())
        }).when(|c| if c.state.hidden.is_empty() { Avail::Blocked("Nothing is hidden".into()) } else { Avail::Ready }),
        cmd("edit.lock", "Lock", "lock", "", "Prevent picking the entity in the viewport.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select an entity")?.to_owned();
            let locked = e.state.locked.contains(&entity);
            act(e, "editor.lock", json!({"entity": entity, "enabled": !locked}))
        }).when(|c| match c.target.entity(c.state) { Some(id) => Avail::Checked(c.state.locked.contains(id)), None => has_entity(c) }),
        cmd("prefab.unpack", "Unpack Prefab", "package", "", "Turn a prefab instance into ordinary entities.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select a prefab instance")?.to_owned();
            apply(e, vec![json!({"cmd": "editor.prefab.unpack", "args": {"entity": entity}})], "Unpack prefab")
        }).when(|c| {
            let Some(id) = c.target.entity(c.state) else { return has_entity(c) };
            let doc = c.store.workspace().doc(&c.state.doc);
            if doc.and_then(|d| d.record(id)).is_some_and(|r| ara_doc::prefab_of(r).is_some()) { Avail::Ready } else { Avail::Blocked("Not a prefab instance".into()) }
        }),
    ]);
    // View
    c.extend([
        later("view.camera.perspective", "Perspective / Orthographic", "camera", "Num5", "The renderer's camera is perspective-only; orthographic projection needs ara-render support."),
        cmd("view.camera.default", "Default View", "video", "", "Return to the default three-quarter perspective.", |_, e, _| camera(e, 0.6, 0.4)),
        cmd("view.camera.top", "Top", "", "Num7", "Look straight down.", |_, e, _| camera(e, 0., 1.54)),
        cmd("view.camera.bottom", "Bottom", "", "Ctrl+Num7", "Look straight up.", |_, e, _| camera(e, 0., -1.54)),
        cmd("view.camera.front", "Front", "", "Num1", "Look along −Z.", |_, e, _| camera(e, 0., 0.)),
        cmd("view.camera.back", "Back", "", "Ctrl+Num1", "Look along +Z.", |_, e, _| camera(e, std::f32::consts::PI, 0.)),
        cmd("view.camera.left", "Left", "", "Ctrl+Num3", "Look along +X.", |_, e, _| camera(e, -std::f32::consts::FRAC_PI_2, 0.)),
        cmd("view.camera.right", "Right", "", "Num3", "Look along −X.", |_, e, _| camera(e, std::f32::consts::FRAC_PI_2, 0.)),
        cmd("view.frame", "Frame Selected", "focus", "F", "Frame the selection, or the scene when nothing is selected.", |s, e, _| {
            if s.graph_visible() { s.frame_graph(e) } else { act(e, "editor.frame", json!({})) }
        }),
        cmd("view.frame_all", "Frame All", "focus", "Home", "Frame every entity.", |_, e, _| {
            let selection = e.state.selection.clone();
            select(e, Vec::new())?;
            act(e, "editor.frame", json!({}))?;
            select(e, selection)
        }),
        cmd("view.camera.speed_up", "Faster Camera", "gauge", "", "Double the fly speed.", |_, e, _| {
            let v = e.state.settings.get("viewport.cameraSpeed").and_then(Value::as_f64).unwrap_or(4.);
            set(e, "viewport.cameraSpeed", json!((v * 2.).min(50.)))
        }),
        cmd("view.camera.speed_down", "Slower Camera", "gauge", "", "Halve the fly speed.", |_, e, _| {
            let v = e.state.settings.get("viewport.cameraSpeed").and_then(Value::as_f64).unwrap_or(4.);
            set(e, "viewport.cameraSpeed", json!((v * 0.5).max(0.5)))
        }),
        later("view.camera.bookmarks", "Bookmarks", "", "", "Camera bookmarks need a per-scene editor sidecar; they are not built yet."),
        later("view.camera.pilot", "Pilot Selected Camera", "camera", "", "Piloting a scene camera needs the Game view through the shared texture (D19)."),
    ]);
    c.extend([
        cmd(
            "view.mode.lit",
            "Lit",
            "sun",
            "",
            "Final lighting.",
            |_, e, _| view(e, json!({"viewMode": "lit"})),
        )
        .when(|c| mode(c, "lit")),
        cmd(
            "view.mode.base_color",
            "Base Color",
            "palette",
            "",
            "Unlit material base color.",
            |_, e, _| view(e, json!({"viewMode": "baseColor"})),
        )
        .when(|c| mode(c, "baseColor")),
        cmd(
            "view.mode.normals",
            "Normals",
            "",
            "",
            "World normals.",
            |_, e, _| view(e, json!({"viewMode": "normals"})),
        )
        .when(|c| mode(c, "normals")),
        cmd(
            "view.mode.depth",
            "Depth",
            "",
            "",
            "Reversed-Z depth.",
            |_, e, _| view(e, json!({"viewMode": "depth"})),
        )
        .when(|c| mode(c, "depth")),
        cmd(
            "view.mode.roughness_metal",
            "Roughness / Metal",
            "",
            "",
            "Red roughness, green metallic.",
            |_, e, _| view(e, json!({"viewMode": "roughnessMetal"})),
        )
        .when(|c| mode(c, "roughnessMetal")),
        cmd(
            "view.mode.entity_ids",
            "Entity IDs",
            "",
            "",
            "Stable pseudocolors per entity.",
            |_, e, _| view(e, json!({"viewMode": "entityIds"})),
        )
        .when(|c| mode(c, "entityIds")),
        cmd(
            "view.mode.shadow_cascades",
            "Shadow Cascades",
            "",
            "",
            "Cascade selection times shadow visibility.",
            |_, e, _| view(e, json!({"viewMode": "shadowCascades"})),
        )
        .when(|c| mode(c, "shadowCascades")),
    ]);
    c.extend([
        later("view.mode.wireframe", "Wireframe", "", "", "ara-render has no wireframe pass yet."),
        later("view.mode.overdraw", "Overdraw", "", "", "ara-render has no overdraw visualisation yet."),
        later("view.mode.gi_debug", "GI Debug", "", "", "GI debug views live in ara-render-fidelity captures; live viewport output is not wired yet."),
        cmd("view.show.grid", "Grid", "grid-2x2", "", "Show the ground grid.", |_, e, _| toggle(e, "viewport.grid")).when(|c| flag(c, "viewport.grid")),
        cmd("view.show.outline", "Selection Outline", "", "", "Outline selected entities.", |_, e, _| toggle(e, "viewport.outline")).when(|c| flag(c, "viewport.outline")),
        cmd("view.show.gizmos", "Transform Gizmo", "move-3d", "", "Show transform handles on the selection.", |_, e, _| view(e, json!({"gizmos": !e.state.hide_gizmos}))).when(|c| Avail::Checked(!c.state.hide_gizmos)),
        cmd("view.show.orientation", "Orientation Widget", "globe", "", "Show the axis widget in the top-right corner.", |_, e, _| view(e, json!({"orientation": !e.state.hide_orientation}))).when(|c| Avail::Checked(!c.state.hide_orientation)),
        later("view.show.bounds", "Bounds", "", "", "Bounds drawing needs mesh bounds from the extraction; it is not wired yet."),
        later("view.show.colliders", "Colliders", "", "", "Collider shapes need an ara-physics debug-draw feed into the editor overlay."),
        later("view.show.stats", "Stats Overlay", "gauge", "", "The viewport Stats overlay arrives with Profiler v2."),
        cmd("view.game", "Game View", "play", "G", "Hide every editor visual in the viewport.", |_, e, _| view(e, json!({"gameView": !e.state.game_view}))).when(|c| Avail::Checked(c.state.game_view)),
        cmd("view.hide_overlays", "Hide Overlays", "eye-off", "Shift+G", "Hide the viewport toolbar and widgets.", |_, e, _| view(e, json!({"hideOverlays": !e.state.hide_overlays}))).when(|c| Avail::Checked(c.state.hide_overlays)),
        cmd("view.fidelity", "Fidelity Rendering", "sun", "", "Switch between ray-traced Fidelity and the Raster preview.", |_, e, _| {
            let next = if e.state.quality { "raster" } else { "fidelity" };
            set(e, "viewport.quality", json!(next))
        }).when(|c| Avail::Checked(c.state.quality)),
        later("view.environment", "Environment Preview", "sun", "", "Time-of-day and weather previews need the sky system in the editor."),
        later("view.layout_quad", "Viewport Layout", "grid-2x2", "", "Several viewports need one render target per view; quad view is not built yet."),
        cmd("window.maximize", "Maximize Panel", "maximize", "Shift+Space", "Fill the window with the active panel; again to restore.", |s, e, t| {
            let panel = panel_target(s, e, t);
            let next = if e.state.maximized.as_deref() == Some(panel.as_str()) { String::new() } else { panel };
            view(e, json!({"maximized": next}))
        }).when(|c| Avail::Checked(c.state.maximized.is_some())),
        cmd("view.fullscreen", "Full Screen", "maximize", "F11", "Toggle full-screen for the main window.", |s, _, _| {
            s.window_request = Some(super::super::WindowRequest::Fullscreen);
            Ok(())
        }),
        cmd("view.zoom_in", "Zoom In", "plus", "Ctrl+=", "Increase UI scale.", |_, e, _| {
            let v = e.state.settings.get("appearance.uiScale").and_then(Value::as_f64).unwrap_or(1.);
            set(e, "appearance.uiScale", json!((((v + 0.1) * 20.).round() / 20.).clamp(0.75, 2.)))
        }),
        cmd("view.zoom_out", "Zoom Out", "minus", "Ctrl+-", "Decrease UI scale.", |_, e, _| {
            let v = e.state.settings.get("appearance.uiScale").and_then(Value::as_f64).unwrap_or(1.);
            set(e, "appearance.uiScale", json!((((v - 0.1) * 20.).round() / 20.).clamp(0.75, 2.)))
        }),
        cmd("view.zoom_reset", "Reset Zoom", "", "Ctrl+0", "Restore 100% UI scale.", |_, e, _| set(e, "appearance.uiScale", json!(1.))),
        cmd("view.density.compact", "Compact", "", "", "22 px rows; the default.", |_, e, _| set(e, "appearance.density", json!("compact"))).when(|c| is(c, "appearance.density", "compact")),
        cmd("view.density.comfortable", "Comfortable", "", "", "26 px rows and roomier controls.", |_, e, _| set(e, "appearance.density", json!("comfortable"))).when(|c| is(c, "appearance.density", "comfortable")),
        cmd("view.theme.dark", "Dark", "moon", "", "Ara Dark.", |_, e, _| set(e, "appearance.theme", json!("dark"))).when(|c| is(c, "appearance.theme", "dark")),
        cmd("view.theme.light", "Light", "sun", "", "Ara Light.", |_, e, _| set(e, "appearance.theme", json!("light"))).when(|c| is(c, "appearance.theme", "light")),
        cmd("view.reduce_motion", "Reduce Motion", "", "", "Turn off decorative transitions.", |_, e, _| toggle(e, "appearance.reduceMotion")).when(|c| flag(c, "appearance.reduceMotion")),
        cmd("view.tool.select", "Select Tool", "mouse-pointer", "Q", "Pick and box-select without handles.", |_, e, _| view(e, json!({"gizmos": false}))).when(|c| Avail::Checked(c.state.hide_gizmos)),
        cmd("view.tool.move", "Move Tool", "move-3d", "W", "Translate along axes or planes.", |_, e, _| {
            view(e, json!({"gizmos": true}))?;
            act(e, "editor.tool", json!({"tool": "translate"}))
        }).when(|c| Avail::Checked(!c.state.hide_gizmos && c.state.tool == crate::state::Tool::Translate)),
        cmd("view.tool.rotate", "Rotate Tool", "rotate-3d", "E", "Rotate around an axis.", |_, e, _| {
            view(e, json!({"gizmos": true}))?;
            act(e, "editor.tool", json!({"tool": "rotate"}))
        }).when(|c| Avail::Checked(!c.state.hide_gizmos && c.state.tool == crate::state::Tool::Rotate)),
        cmd("view.tool.scale", "Scale Tool", "scaling", "R", "Scale along an axis or uniformly.", |_, e, _| {
            view(e, json!({"gizmos": true}))?;
            act(e, "editor.tool", json!({"tool": "scale"}))
        }).when(|c| Avail::Checked(!c.state.hide_gizmos && c.state.tool == crate::state::Tool::Scale)),
        cmd("view.space", "World / Local Space", "globe", "", "Toggle world or local handle axes.", |_, e, _| {
            act(e, "editor.tool", json!({"tool": e.state.tool, "local": !e.state.local}))
        }).when(|c| Avail::Checked(c.state.local)),
        cmd("view.pivot", "Pivot / Centre", "crosshair", "", "Toggle the handle between each pivot and the selection centre.", |_, e, _| {
            act(e, "editor.tool", json!({"tool": e.state.tool, "centre": !e.state.centre}))
        }).when(|c| Avail::Checked(c.state.centre)),
    ]);
    // Create
    c.extend([
        cmd("create.empty", "Empty Entity", "layers", "Ctrl+Shift+N", "Create an entity with only a Transform.", |_, e, _| create(e, &Target::None, "Entity", json!({}), false)).when(scene_doc),
        cmd("create.empty_child", "Empty Child", "layers", "Alt+Shift+N", "Create an empty entity under the selection.", |_, e, t| create(e, t, "Entity", json!({}), true)).when(has_entity),
        later("create.group", "Group Selection", "layers", "Ctrl+G", "Grouping needs an atomic reparent-under-new-parent command; reparent by dragging for now."),
        cmd("create.at_cursor", "Create…", "plus", "Shift+A", "Search every Create command.", |s, e, _| s.open_palette(e, ">Create ")).when(scene_doc),
        cmd("create.cube", "Cube", "box", "", "Box mesh, 1 m.", |_, e, t| shape(e, t, "Cube", "box")).when(|c| with_components(c, &["MeshRenderer"])),
        cmd("create.sphere", "Sphere", "circle", "", "Sphere mesh, 1 m diameter.", |_, e, t| shape(e, t, "Sphere", "sphere")).when(|c| with_components(c, &["MeshRenderer"])),
        cmd("create.plane", "Plane", "square", "", "Upward-facing quad, 1 m.", |_, e, t| shape(e, t, "Plane", "plane")).when(|c| with_components(c, &["MeshRenderer"])),
        cmd("create.capsule", "Capsule", "pill", "", "Upright capsule.", |_, e, t| shape(e, t, "Capsule", "capsule")).when(|c| with_components(c, &["MeshRenderer"])),
        later("create.cylinder", "Cylinder", "", "", "MeshShape offers box, sphere, capsule and plane; import a cylinder model for now."),
        cmd("create.mesh", "Mesh from Asset…", "package", "", "Pick a model in Assets and drop it into the scene.", |_, e, _| focus(e, "assets")),
        cmd("create.light.sun", "Sun", "sun", "", "Directional light with cascaded shadows.", |_, e, t| create(e, t, "Sun", json!({"Light": {"kind": "directional", "intensity": 3}, "Transform": {"position": [0, 6, 0], "rotation": [-50, 30, 0]}}), false)).when(|c| with_components(c, &["Light"])),
        cmd("create.light.point", "Point Light", "lightbulb", "", "Omnidirectional light.", |_, e, t| create(e, t, "Point light", json!({"Light": {"kind": "point"}}), matches!(t, Target::Entity(_)))).when(|c| with_components(c, &["Light"])),
        cmd("create.light.spot", "Spot Light", "lightbulb", "", "Cone light along −Z.", |_, e, t| create(e, t, "Spot light", json!({"Light": {"kind": "spot"}}), false)).when(|c| with_components(c, &["Light"])),
        cmd("create.light.sky", "Sky & Atmosphere", "sun", "", "Procedural sky and exposure.", |_, e, t| create(e, t, "Sky", json!({"Environment": {}}), false)).when(|c| with_components(c, &["Environment"])),
        later("create.light.probe", "Reflection Probe", "", "", "Probes are automatic in the fidelity renderer; authored probes are not in the schema."),
        cmd("create.camera", "Camera", "camera", "", "Viewpoint along −Z.", |_, e, t| create(e, t, "Camera", json!({"Camera": {}}), false)).when(|c| with_components(c, &["Camera"])),
        cmd("create.vfx", "VFX System", "sparkles", "", "Entity that plays a VFX graph.", |_, e, t| create(e, t, "VFX", json!({"VisualEffect": {}}), false)).when(|c| with_components(c, &["VisualEffect"])),
        later("create.decal", "Decal", "", "", "Decals are not in the renderer schema yet."),
        later("create.fog", "Fog Volume", "", "", "Fog volumes are not in the renderer schema yet."),
        cmd("create.physics.body", "Rigid Body", "box", "", "Box with a dynamic body and collider.", |_, e, t| create(e, t, "Rigid body", json!({"MeshRenderer": {}, "RigidBody": {}, "Collider": {}}), false)).when(|c| with_components(c, &["RigidBody", "Collider", "MeshRenderer"])),
        cmd("create.physics.collider", "Collider", "box", "", "Static collision shape.", |_, e, t| create(e, t, "Collider", json!({"Collider": {}}), false)).when(|c| with_components(c, &["Collider"])),
        cmd("create.physics.character", "Character Controller", "user", "", "Kinematic character capsule.", |_, e, t| create(e, t, "Character", json!({"CharacterController": {}, "MeshRenderer": {"shape": "capsule", "size": [0.6, 1.8, 0.6]}}), false)).when(|c| with_components(c, &["CharacterController", "MeshRenderer"])),
        later("create.physics.trigger", "Trigger Volume", "", "", "Trigger volumes need a sensor flag on Collider; it is not in the schema yet."),
        cmd("create.audio.source", "Audio Source", "volume-2", "", "Positional sound emitter.", |_, e, t| create(e, t, "Audio source", json!({"AudioSource": {}}), false)).when(|c| with_components(c, &["AudioSource"])),
        cmd("create.audio.listener", "Listener", "ear", "", "Where the mix is heard from.", |_, e, t| create(e, t, "Listener", json!({"AudioListener": {}}), false)).when(|c| with_components(c, &["AudioListener"])),
        cmd("create.nav.volume", "Nav Mesh Surface", "map", "", "Bakes a navigation mesh.", |_, e, t| create(e, t, "Nav surface", json!({"NavMeshSurface": {}}), false)).when(|c| with_components(c, &["NavMeshSurface"])),
        cmd("create.nav.link", "Nav Link", "link", "", "Off-mesh connection.", |_, e, t| create(e, t, "Nav link", json!({"NavLink": {}}), false)).when(|c| with_components(c, &["NavLink"])),
        later("create.world.terrain", "Terrain", "", "", "Terrain authoring in the editor arrives with Terrain Tools."),
        later("create.world.foliage", "Foliage", "", "", "Foliage painting arrives with Terrain Tools."),
        cmd("create.2d.sprite", "Sprite", "image", "", "2D sprite.", |_, e, t| create(e, t, "Sprite", json!({"Sprite": {}}), false)).when(|c| with_components(c, &["Sprite"])),
        cmd("create.2d.tilemap", "Tilemap", "grid-2x2", "", "2D tile grid.", |_, e, t| create(e, t, "Tilemap", json!({"Tilemap": {}}), false)).when(|c| with_components(c, &["Tilemap"])),
        later("create.ui.screen", "Screen UI Document", "", "", "UI document components are not in the runtime schema yet."),
        later("create.ui.world", "World-space UI", "", "", "World-space UI components are not in the runtime schema yet."),
        cmd("create.gameplay.player", "Player", "user", "", "Capsule driven by PlayerControl.", |_, e, t| create(e, t, "Player", json!({"PlayerControl": {}, "MeshRenderer": {"shape": "capsule", "size": [0.6, 1.8, 0.6]}}), false)).when(|c| with_components(c, &["PlayerControl", "MeshRenderer"])),
        later("create.gameplay.spawn", "Spawn Point", "", "", "Spawn points are not in the gameplay schema yet."),
        later("create.prefab", "Prefab from Selection", "package", "", "Creating a prefab document from entities needs a typed prefab.create command."),
    ]);
    // Assets
    c.extend([
        cmd("assets.create.folder", "Folder", "folder", "", "Create a folder in the current asset folder.", |s, e, _| s.new_folder(e)),
        cmd("assets.create.scene", "Scene", "box", "", "Create an empty scene.", |s, e, _| s.new_scene(e)),
        cmd("assets.create.material", "Material", "circle", "", "Create a material graph and open it.", |s, e, _| s.create_resource(e, "material")),
        cmd("assets.create.vfx", "VFX System", "sparkles", "", "Create a VFX graph and open it.", |s, e, _| s.create_resource(e, "vfx")),
        cmd("assets.create.animation", "Animation Controller", "activity", "", "Create an animation controller and open it.", |s, e, _| s.create_resource(e, "animation")),
        later("assets.create.script", "C# Script", "file-code", "", "Create scripts in your IDE; the C# host hot-reloads them."),
        later("assets.create.prefab", "Prefab", "package", "", "Prefab documents need a typed prefab.create command."),
        cmd("assets.reimport", "Reimport", "refresh-cw", "", "Run the importer again for the selected asset.", |s, e, t| {
            let path = s.asset_target(e, t).ok_or("select an asset")?;
            s.forget_thumbnail(&path);
            act(e, "editor.asset.reimport", json!({"path": path}))
        }).when(has_asset),
        later("assets.reimport_changed", "Reimport All Changed", "refresh-cw", "", "Changed sources reimport automatically when the host sees them on disk."),
        cmd("assets.open", "Open", "external-link", "Enter", "Open the asset in its editor, or the folder.", |s, e, t| s.open_asset(e, t)).when(has_asset),
        cmd("assets.show", "Show in Assets", "folder", "Ctrl+B", "Reveal the selected entity's model, or the asset, in Assets.", |s, e, t| s.show_in_assets(e, t)),
        cmd("assets.reveal", "Show in Explorer", "folder", "", "Open the containing folder in the OS file manager.", |s, e, t| {
            let path = s.asset_target(e, t).unwrap_or_else(|| e.state.asset_folder.clone());
            super::super::open_external(&e.host.root().join(path))
        }),
        cmd("assets.copy_reference", "Copy Reference", "link", "", "Copy the asset's `@path#id`.", |s, e, t| {
            let path = s.asset_target(e, t).ok_or("select an asset")?;
            let text = s.asset_reference(e, &path).unwrap_or_else(|| format!("@{path}"));
            s.copy_text(text);
            Ok(())
        }).when(has_asset),
        cmd("assets.references", "Find References", "search", "", "Show what uses the asset and what it uses.", |s, e, t| {
            let path = s.asset_target(e, t).ok_or("select an asset")?;
            act(e, "editor.workspace", json!({"asset": path}))?;
            focus(e, "inspector")
        }).when(has_asset),
        later("assets.rename", "Rename", "pencil", "F2", "Asset renames must rewrite every `@path#id` reference; the typed move command is not built yet."),
        later("assets.duplicate", "Duplicate", "copy", "", "Asset duplication needs new stable IDs from a typed asset.copy command."),
        later("assets.delete", "Delete", "trash", "", "Deleting assets needs the reference-count warning from asset.delete; not built yet."),
        later("assets.validate", "Validate Project", "circle-check", "", "Run `ara check` for now; the in-editor validation report arrives with Problems."),
        cmd("assets.rebuild_thumbnails", "Rebuild Thumbnails", "image", "", "Re-render every asset thumbnail off the UI thread.", |s, _, _| {
            s.rebuild_thumbnails();
            Ok(())
        }),
        later("assets.clear_cache", "Clear Cache", "trash", "", "Clearing caches while jobs run is unsafe; delete .ara/cache with the editor closed."),
        later("assets.import_defaults", "Import Defaults…", "settings-2", "", "Per-type import defaults arrive with the Assets settings page."),
    ]);
    // Play
    c.extend([
        cmd(
            "play.start",
            "Play",
            "play",
            "F5",
            "Play the scene with the project's play settings; resumes when paused.",
            |_, e, _| {
                if e.state.playing == Some(false) {
                    play(e, "play.resume", json!({}))?;
                } else {
                    let mut params = play_params(e);
                    params["scene"] = json!(e.state.doc);
                    play(e, "play.start", params)?;
                }
                view(e, json!({"play": "playing"}))
            },
        )
        .labelled(|c| {
            if c.state.playing == Some(false) {
                "Resume".into()
            } else {
                "Play".into()
            }
        }),
        cmd(
            "play.pause",
            "Pause",
            "pause",
            "F6",
            "Pause the running world.",
            |_, e, _| {
                play(e, "play.pause", json!({}))?;
                view(e, json!({"play": "paused"}))
            },
        )
        .when(|c| {
            if c.state.playing == Some(true) {
                Avail::Ready
            } else {
                Avail::Blocked("Nothing is playing".into())
            }
        }),
        cmd(
            "play.step",
            "Step One Tick",
            "step-forward",
            "F7",
            "Advance a paused world by one fixed tick (F10 is the Windows menu key).",
            |_, e, _| play(e, "play.step", json!({"ticks": 1})),
        )
        .when(|c| {
            if c.state.playing.is_some() {
                Avail::Ready
            } else {
                Avail::Blocked("Start Play first".into())
            }
        }),
        cmd(
            "play.stop",
            "Stop",
            "square",
            "Shift+F5",
            "Stop every player process.",
            |_, e, _| {
                play(e, "play.stop", json!({}))?;
                view(e, json!({"play": "stopped"}))
            },
        )
        .when(|c| {
            if c.state.playing.is_some() {
                Avail::Ready
            } else {
                Avail::Blocked("Nothing is playing".into())
            }
        }),
        cmd(
            "play.restart",
            "Restart",
            "rotate-ccw",
            "Ctrl+Shift+F5",
            "Restart the world from tick 0.",
            |_, e, _| play(e, "play.restart", json!({})),
        )
        .when(|c| {
            if c.state.playing.is_some() {
                Avail::Ready
            } else {
                Avail::Blocked("Nothing is playing".into())
            }
        }),
        later(
            "play.simulate",
            "Simulate",
            "play",
            "Alt+F5",
            "Simulating without a player needs the in-editor Game view (D19 shared texture).",
        ),
        later(
            "play.eject",
            "Eject / Possess",
            "",
            "F8",
            "Eject needs the in-editor Game view (D19 shared texture).",
        ),
        cmd(
            "play.mode.local",
            "Local",
            "user",
            "",
            "One local player.",
            |_, e, _| set(e, "play.mode", json!("local")),
        )
        .when(|c| is(c, "play.mode", "local")),
        cmd(
            "play.mode.host",
            "Host + Clients",
            "users",
            "",
            "A listen host plus client processes.",
            |_, e, _| set(e, "play.mode", json!("host")),
        )
        .when(|c| is(c, "play.mode", "host")),
        cmd(
            "play.mode.dedicated",
            "Dedicated + Clients",
            "server",
            "",
            "A dedicated server plus client processes.",
            |_, e, _| set(e, "play.mode", json!("dedicated")),
        )
        .when(|c| is(c, "play.mode", "dedicated")),
        cmd(
            "play.clients.more",
            "Add Client",
            "plus",
            "",
            "One more client process.",
            |_, e, _| {
                let n = e
                    .state
                    .settings
                    .get("play.clients")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.);
                set(e, "play.clients", json!((n + 1.).min(16.)))
            },
        )
        .labelled(|c| {
            format!(
                "Add Client ({} now)",
                setting(c, "play.clients").as_f64().unwrap_or(1.)
            )
        }),
        cmd(
            "play.clients.fewer",
            "Remove Client",
            "minus",
            "",
            "One fewer client process.",
            |_, e, _| {
                let n = e
                    .state
                    .settings
                    .get("play.clients")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.);
                set(e, "play.clients", json!((n - 1.).max(0.)))
            },
        ),
        cmd(
            "play.start_paused",
            "Start Paused",
            "pause",
            "",
            "Begin at tick 0 and step explicitly.",
            |_, e, _| toggle(e, "play.startPaused"),
        )
        .when(|c| flag(c, "play.startPaused")),
        later(
            "play.network_profile",
            "Network Profile",
            "wifi",
            "",
            "Per-client network conditions need ara-net link shaping exposed through play.start.",
        ),
        cmd(
            "play.review",
            "Review Kept Changes…",
            "bot",
            "",
            "Open the review queue.",
            |_, e, _| focus(e, "agents"),
        ),
        cmd(
            "play.settings",
            "Play Settings…",
            "settings-2",
            "",
            "Open Networking & Play settings.",
            |s, e, _| s.open_settings(e, "project", "Networking & Play"),
        ),
    ]);
    // Tools
    c.extend([
        cmd("window.profiler", "Profiler", "gauge", "Alt+8", "Record and compare editor frames.", |_, e, _| act(e, "editor.layout.show", json!({"panel": "profiler"}))).when(|c| panel_open(c, "profiler")),
        later("tools.gpu_capture", "GPU Frame Capture", "camera", "", "GPU captures run through RenderDoc for now; the integrated capture viewer is not built."),
        later("tools.why", "Diagnostics: Why?", "search", "", "Run `ara why` (MCP: ara_why); the in-editor Why panel arrives with Problems."),
        later("tools.references", "Reference Viewer", "link", "", "The reference graph view arrives with Asset browser v2."),
        later("tools.table", "Table View…", "grid-2x2", "", "Spreadsheet bulk editing is backlog item 26."),
        later("tools.localization", "Localization", "", "", "ara-l10n tables are edited as files; the editor panel is not built."),
        later("tools.input_actions", "Input Actions", "", "", "Input action maps are edited as files; the editor panel is not built."),
        later("tools.physics_debugger", "Physics Debugger", "", "", "Needs an ara-physics debug-draw feed into the editor."),
        later("tools.terrain", "Terrain Tools", "", "", "Terrain sculpting and painting tools are not built yet."),
        later("tools.ui_designer", "UI Designer", "", "", "The UI designer over ara-ui-document ops is not built yet."),
        later("tools.cs.solution", "Open Solution in IDE", "file-code", "", "Generate the solution with `ara script` and open it in your IDE."),
        later("tools.cs.reload", "Reload Scripts", "refresh-cw", "Ctrl+R", "C# reloads automatically when sources change; a manual trigger is not exposed yet."),
        cmd("window.agents", "Agents Panel", "bot", "Alt+6", "Review agent changes and the apply policy.", |_, e, _| act(e, "editor.layout.show", json!({"panel": "agents"}))).when(|c| panel_open(c, "agents")),
        cmd("agents.policy.immediately", "Apply Immediately", "", "", "Agent changes apply at the next frame.", |_, e, _| set(e, "agents.applyPolicy", json!("immediately"))).when(|c| is(c, "agents.applyPolicy", "immediately")),
        cmd("agents.policy.idle", "Apply When Idle", "", "", "Agent changes wait while you are typing or dragging.", |_, e, _| set(e, "agents.applyPolicy", json!("whenIdle"))).when(|c| is(c, "agents.applyPolicy", "whenIdle")),
        cmd("agents.policy.approval", "Apply On Approval", "", "", "Agent changes wait in Agents for your approval.", |_, e, _| set(e, "agents.applyPolicy", json!("onApproval"))).when(|c| is(c, "agents.applyPolicy", "onApproval")),
        cmd("agents.connect", "Connect an Agent (MCP)…", "bot", "", "Show how to connect Claude Code, Codex or another MCP client.", |s, e, _| {
            s.dialog = Some(super::super::Dialog::connect(e));
            Ok(())
        }),
        later("tools.vcs", "Version Control", "", "", "Use git; the in-editor changes view is not built yet."),
        later("tools.plugins", "Plugins…", "", "", "Plugin packages (D22) are not built yet."),
        cmd("tools.terminal", "Open Terminal Here", "terminal", "", "Open a terminal at the project root.", |_, e, _| super::super::open_terminal(e.host.root())),
    ]);
    // Window
    c.extend([
        cmd(
            "layout.default",
            "Default",
            "layout-grid",
            "",
            "Hierarchy, Scene, Inspector and a bottom Assets/Console strip.",
            |_, e, _| layout(e, "default"),
        ),
        cmd(
            "layout.level_design",
            "Level Design",
            "layout-grid",
            "",
            "Scene-first: Hierarchy and Inspector share the right column.",
            |_, e, _| layout(e, "levelDesign"),
        ),
        cmd(
            "layout.materials",
            "Materials & VFX",
            "layout-grid",
            "",
            "Graph documents with Assets on the left and Inspector on the right.",
            |_, e, _| layout(e, "materials"),
        ),
        cmd(
            "layout.animation",
            "Animation",
            "layout-grid",
            "",
            "Animation graph with Scene preview and Inspector.",
            |_, e, _| layout(e, "animation"),
        ),
        cmd(
            "layout.profiling",
            "Profiling",
            "layout-grid",
            "",
            "Profiler across the window, Scene above it.",
            |_, e, _| layout(e, "profiling"),
        ),
        cmd(
            "layout.laptop",
            "Laptop (1440 × 900)",
            "layout-grid",
            "",
            "Narrow side columns; Assets and Console tabbed with the Inspector.",
            |_, e, _| layout(e, "laptop"),
        ),
    ]);
    c.extend([
        cmd(
            "layout.save",
            "Save Layout As…",
            "plus",
            "",
            "Save the current arrangement, including floating windows.",
            |s, _, _| {
                s.dialog = Some(super::super::Dialog::save_layout());
                Ok(())
            },
        ),
        cmd(
            "layout.load",
            "Load Layout",
            "layout-grid",
            "",
            "Apply a saved layout by name (target text).",
            |_, e, t| {
                let Target::Text(name) = t else {
                    return Err("choose a layout".into());
                };
                act(e, "editor.layout.apply", json!({"name": name}))
            },
        ),
        later(
            "layout.saved",
            "Manage Layouts…",
            "",
            "",
            "Rename and delete layouts in .ara/layouts for now; a manager page is not built.",
        ),
        cmd(
            "layout.reset",
            "Reset Layout",
            "rotate-ccw",
            "",
            "Restore the default arrangement and close floating windows.",
            |_, e, _| act(e, "editor.layout.reset", json!({})),
        ),
        cmd(
            "window.show_panel",
            "Show Panel",
            "",
            "",
            "Activate a panel tab (target panel).",
            |_, e, t| {
                let Target::Panel(panel) = t else {
                    return Err("choose a panel".into());
                };
                focus(e, panel)
            },
        ),
        cmd(
            "window.hierarchy",
            "Hierarchy",
            "list-tree",
            "Alt+1",
            "Show the Hierarchy.",
            |_, e, _| act(e, "editor.layout.show", json!({"panel": "hierarchy"})),
        )
        .when(|c| panel_open(c, "hierarchy")),
        cmd(
            "window.inspector",
            "Inspector",
            "sliders-horizontal",
            "Alt+2",
            "Show the Inspector.",
            |_, e, _| act(e, "editor.layout.show", json!({"panel": "inspector"})),
        )
        .when(|c| panel_open(c, "inspector")),
        cmd(
            "window.assets",
            "Assets",
            "folder",
            "Alt+3 / Ctrl+Space",
            "Show the asset browser.",
            |_, e, _| act(e, "editor.layout.show", json!({"panel": "assets"})),
        )
        .when(|c| panel_open(c, "assets")),
        cmd(
            "window.console",
            "Console",
            "terminal",
            "Alt+4 / Ctrl+`",
            "Show the console and problems.",
            |_, e, _| act(e, "editor.layout.show", json!({"panel": "console"})),
        )
        .when(|c| panel_open(c, "console")),
        later(
            "window.history",
            "History",
            "clock",
            "Alt+7",
            "The History panel arrives with the Phase 2 tool rebuilds.",
        ),
        cmd(
            "window.float",
            "Move Panel to New Window",
            "external-link",
            "",
            "Tear the panel off into its own OS window.",
            |s, e, t| move_panel(s, e, t, "float"),
        )
        .when(|c| match c.target {
            Target::Panel(p) if p == "scene" => Avail::Blocked(
                "The Scene viewport stays in the main window; float tool panels and graphs".into(),
            ),
            _ => Avail::Ready,
        }),
        cmd(
            "window.move.left",
            "Left",
            "panel-left",
            "",
            "Dock the panel in the left column.",
            |s, e, t| move_panel(s, e, t, "left"),
        ),
        cmd(
            "window.move.right",
            "Right",
            "panel-right",
            "",
            "Dock the panel in the right column.",
            |s, e, t| move_panel(s, e, t, "right"),
        ),
        cmd(
            "window.move.bottom",
            "Bottom",
            "panel-bottom",
            "",
            "Dock the panel in the bottom area.",
            |s, e, t| move_panel(s, e, t, "bottom"),
        ),
        cmd(
            "window.move.center",
            "Document Area",
            "square",
            "",
            "Dock the panel with the Scene and graph documents.",
            |s, e, t| move_panel(s, e, t, "view"),
        ),
        cmd(
            "window.close_panel",
            "Close Panel",
            "x",
            "Shift+Esc",
            "Hide the panel; reopen it from Window.",
            |s, e, t| {
                let panel = panel_target(s, e, t);
                act(e, "editor.layout.close", json!({"panel": panel}))
            },
        )
        .when(|c| match c.target {
            Target::Panel(p) if p == "scene" => {
                Avail::Blocked("The Scene view cannot be closed".into())
            }
            _ => Avail::Ready,
        }),
        later(
            "window.auto_hide",
            "Auto-hide Panel",
            "",
            "",
            "Auto-hide rails and drawers are Docking v2 step 2.",
        ),
        cmd(
            "window.next_document",
            "Next Document",
            "",
            "Ctrl+Tab",
            "Activate the next tab in the document area.",
            |_, e, _| act(e, "editor.layout.next", json!({})),
        ),
        cmd(
            "window.gather",
            "Gather Floating Windows",
            "minimize",
            "",
            "Move every floating window onto the main window's monitor.",
            |s, _, _| {
                s.window_request = Some(super::super::WindowRequest::Gather);
                Ok(())
            },
        )
        .when(|c| {
            if c.state.floating.is_empty() {
                Avail::Blocked("No floating windows".into())
            } else {
                Avail::Ready
            }
        }),
        cmd(
            "window.focus_window",
            "Focus Window",
            "",
            "",
            "Raise a floating window.",
            |s, _, t| {
                let Target::Window(id) = t else {
                    return Err("choose a window".into());
                };
                s.window_request = Some(super::super::WindowRequest::Raise(id.clone()));
                Ok(())
            },
        ),
    ]);
    // Help
    c.extend([
        cmd(
            "help.search",
            "Search Commands & Docs",
            "search",
            "F1",
            "Search commands with their descriptions.",
            |s, e, _| s.open_palette(e, "?"),
        ),
        cmd(
            "help.docs",
            "Documentation",
            "book-open",
            "",
            "Open the engine documentation folder.",
            |_, e, _| super::super::open_docs(e),
        ),
        cmd(
            "help.shortcuts",
            "Keyboard Shortcuts Reference",
            "command",
            "",
            "Open the Keyboard Shortcuts page.",
            |s, e, _| s.open_settings(e, "user", "Keyboard shortcuts"),
        ),
        later(
            "help.cs_api",
            "C# API Reference",
            "file-code",
            "",
            "Generate it with `ara docs` (writes .ara/docs).",
        ),
        later(
            "help.agent_guide",
            "Agent Guide",
            "bot",
            "",
            "The project's AGENTS.md is the agent guide; open it from Assets.",
        ),
        later(
            "help.report",
            "Report a Problem…",
            "",
            "",
            "Diagnostics bundles come with the diagnostics bus (`ara why`).",
        ),
        cmd(
            "help.logs",
            "Open Logs Folder",
            "folder",
            "",
            "Open the project's .ara folder.",
            |_, e, _| {
                let path = e.host.root().join(".ara");
                super::super::open_external(&path)
            },
        ),
        cmd(
            "help.system",
            "System Info",
            "gauge",
            "",
            "GPU, driver and Vulkan details.",
            |s, _, _| {
                s.dialog = Some(super::super::Dialog::system(&s.device_info));
                Ok(())
            },
        ),
        cmd(
            "help.about",
            "About Ara",
            "info",
            "",
            "Version and licenses.",
            |s, _, _| {
                s.dialog = Some(super::super::Dialog::about());
                Ok(())
            },
        ),
    ]);
    // Context-only commands.
    c.extend([
        cmd("field.copy", "Copy Value", "copy", "", "Copy the field's value as JSON.", |s, e, t| {
            let Target::Field(path) = t else { return Err("choose a field".into()) };
            let value = s.fields.get(path).map(|f| f.value.clone()).ok_or("field not visible")?;
            s.field_clipboard = Some(value.clone());
            let _ = e;
            s.copy_text(value.to_string());
            Ok(())
        }),
        cmd("field.paste", "Paste Value", "clipboard", "", "Paste a copied value into the field.", |s, e, t| {
            let Target::Field(path) = t else { return Err("choose a field".into()) };
            let value = s.field_clipboard.clone().ok_or("copy a value first")?;
            apply(e, crate::inspector::edit_commands(&e.state.selection, path, &value), "Paste value")
        }).when(|c| if c.clipboard_value { Avail::Ready } else { Avail::Blocked("Copy a value first".into()) }),
        cmd("field.set", "Set Value", "", "", "Set a field on the selection (target `Path=json`).", |_, e, t| {
            let Target::Text(text) = t else { return Err("expected Path=value".into()) };
            let (path, value) = text.split_once('=').ok_or("expected Path=value")?;
            let value: Value = serde_json::from_str(value)?;
            apply(e, crate::inspector::edit_commands(&e.state.selection, path, &value), &format!("Set {path}"))
        }).when(has_entity),
        cmd("field.reset", "Reset to Default", "undo-2", "", "Reset to the default or prefab value.", |_, e, t| {
            let Target::Field(path) = t else { return Err("choose a field".into()) };
            let commands = e.state.selection.iter().map(|entity| json!({"cmd": "editor.property.reset", "args": {"entity": entity, "path": path}})).collect();
            apply(e, commands, "Reset value")
        }),
        cmd("field.copy_path", "Copy Property Path", "link", "", "Copy `@scene#entity/Component.field`.", |s, e, t| {
            let Target::Field(path) = t else { return Err("choose a field".into()) };
            let entity = e.state.selection.first().ok_or("select an entity")?.clone();
            s.copy_text(format!("{}/{path}", reference(e, &entity)));
            Ok(())
        }),
        cmd("field.why", "Why This Value?", "search", "", "Show where the value comes from.", |s, _, t| {
            let Target::Field(path) = t else { return Err("choose a field".into()) };
            s.dialog = Some(super::super::Dialog::provenance(s.fields.get(path).ok_or("field not visible")?));
            Ok(())
        }),
        cmd("component.reset", "Reset Component", "undo-2", "", "Restore every field of the component to its default.", |_, e, t| {
            let Target::Component(name) = t else { return Err("choose a component".into()) };
            let mut commands = Vec::new();
            for entity in &e.state.selection {
                commands.push(json!({"cmd": "component.remove", "args": {"entity": entity, "component": name}}));
                commands.push(json!({"cmd": "component.add", "args": {"entity": entity, "component": name}}));
            }
            apply(e, commands, &format!("Reset {name}"))
        }).when(|c| if matches!(c.target, Target::Component(n) if n == "Transform") { Avail::Blocked("Use Transform › Reset Position, Rotation and Scale".into()) } else { Avail::Ready }),
        cmd("component.remove", "Remove Component", "trash", "", "Remove the component from the selection.", |_, e, t| {
            let Target::Component(name) = t else { return Err("choose a component".into()) };
            let commands = e.state.selection.iter().map(|entity| json!({"cmd": "component.remove", "args": {"entity": entity, "component": name}})).collect();
            apply(e, commands, &format!("Remove {name}"))
        }),
        later("component.copy", "Copy Component", "copy", "", "Component clipboard arrives with Inspector v2."),
        cmd("component.add", "Add Component…", "plus", "", "Search components to add.", |s, e, _| {
            s.palette_command = Some("@components".into());
            act(e, "editor.palette", json!({"open": true}))
        }).when(has_entity),
        cmd("graph.delete", "Delete Nodes", "trash", "Del", "Delete the selected graph nodes.", |s, e, t| s.delete_graph_nodes(e, t)).when(graph_open),
        cmd("graph.frame", "Frame Graph", "focus", "", "Fit the graph in the canvas.", |s, e, _| s.frame_graph(e)).when(graph_open),
        cmd("graph.add", "Add Node…", "plus", "Space", "Search nodes to add at the pointer.", |_, e, _| act(e, "editor.canvas", json!({"add": true}))).when(graph_open),
        cmd("graph.comment", "Add Comment", "file-plus", "", "Add a comment at the pointer.", |s, e, _| s.edit_graph_layout(e, "comment")).when(graph_open),
        cmd("graph.group", "Group Selection", "layout-grid", "", "Group the selected nodes.", |s, e, _| s.edit_graph_layout(e, "group")).when(graph_open),
        cmd("graph.reroute", "Add Reroute", "git-compare", "", "Add a reroute point to a wire.", |s, e, _| s.edit_graph_layout(e, "reroute")).when(graph_open),
        cmd("graph.select_all", "Select All Nodes", "", "", "Select every node.", |s, e, _| s.select_all_nodes(e)).when(graph_open),
        cmd("vfx.module.up", "Move Module Up", "chevron-up", "Alt+Up", "Move the module earlier in its stage.", |s, e, t| vfx_module(s, e, t, "up")).when(vfx_module_target),
        cmd("vfx.module.down", "Move Module Down", "chevron-down", "Alt+Down", "Move the module later in its stage.", |s, e, t| vfx_module(s, e, t, "down")).when(vfx_module_target),
        cmd("vfx.module.duplicate", "Duplicate Module", "copy", "", "Insert a copy after the module.", |s, e, t| vfx_module(s, e, t, "duplicate")).when(vfx_module_target),
        cmd("vfx.module.delete", "Delete Module", "trash", "", "Remove the module from its stage.", |s, e, t| vfx_module(s, e, t, "delete")).when(vfx_module_target),
        cmd("console.copy", "Copy Message", "copy", "", "Copy the diagnostic text.", |s, _, t| {
            let Target::Text(text) = t else { return Err("choose a message".into()) };
            s.copy_text(text.clone());
            Ok(())
        }),
        later("console.clear", "Clear", "trash", "", "Diagnostics clear when their cause is fixed; a log stream with Clear arrives with Console v2."),
        cmd("profiler.record", "Record", "circle-dot", "", "Start recording editor frames.", |s, e, _| s.profiler_record(e)),
        cmd("profiler.freeze", "Freeze Capture", "pause", "", "Freeze and save the current trace.", |_, e, _| act(e, "editor.profile.freeze", json!({}))),
        cmd("profiler.baseline", "Set Baseline", "flag", "", "Freeze the trace as the comparison baseline.", |_, e, _| act(e, "editor.profile.freeze", json!({"baseline": true}))),
        cmd("profiler.compare", "Compare", "git-compare", "", "Compare the frozen capture with the baseline.", |_, e, _| act(e, "editor.workspace", json!({"compare": !e.state.profiler_compare})))
            .when(|c| Avail::Checked(c.state.profiler_compare)),
        cmd("settings.close", "Close Settings", "x", "", "Close the Settings window.", |_, e, _| act(e, "editor.layout.close", json!({"panel": "settings"}))),
    ]);
    c
}
