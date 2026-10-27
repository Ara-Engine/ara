//! The registry table. Commands that are not built yet stay registered with the
//! reason, so menus explain what is missing instead of hiding it.
use super::{Avail, BuildState, Ctx, Run, RunFn, Spec, Target};
use crate::{Editor, shell::Shell};
use ara_render_fidelity::presets::QualityPreset;
use serde_json::{Value, json};
use std::sync::LazyLock;

pub(super) static COMMANDS: LazyLock<Vec<Spec>> = LazyLock::new(build);

/// A Scene viewport quality choice: it sets the user preference `viewport.quality`
/// (`EditorState::render_quality`), checked while active.
macro_rules! viewport_quality {
    ($id:literal, $title:literal, $value:literal, $summary:literal) => {
        cmd($id, $title, "", "", $summary, |_, e, _| {
            set(e, "viewport.quality", json!($value))
        })
        .when(|c| is(c, "viewport.quality", $value))
    };
}

/// A Hierarchy type filter: it writes `query` into the Hierarchy search, so the
/// filter stays visible and editable there.
macro_rules! hierarchy_filter {
    ($id:literal, $title:literal, $icon:literal, $query:literal) => {
        cmd(
            $id,
            $title,
            $icon,
            "",
            "Show only these entities in the Hierarchy; its search box shows the filter.",
            |_, e, _| act(e, "editor.search", json!({"panel": "hierarchy", "query": $query})),
        )
        .when(|c| {
            Avail::Checked(c.state.searches.get("hierarchy").map_or("", String::as_str) == $query)
        })
    };
}

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
fn in_instance(c: &Ctx) -> Avail {
    match c.target.entity(c.state) {
        Some(entity)
            if crate::shell::prefab_ops::instance(c.store, &c.state.doc, entity).is_some() =>
        {
            Avail::Ready
        }
        Some(_) => Avail::Blocked(
            "Not a prefab instance; use Create > Prefab from Selection to make one".into(),
        ),
        None => Avail::Blocked("Select a prefab instance first".into()),
    }
}
fn has_overrides(c: &Ctx) -> Avail {
    match c
        .target
        .entity(c.state)
        .and_then(|e| crate::shell::prefab_ops::instance(c.store, &c.state.doc, e))
    {
        Some(found) if !found.overrides.is_empty() => Avail::Ready,
        Some(_) => {
            Avail::Blocked("This instance has no overrides; edit a field to make one".into())
        }
        None => in_instance(c),
    }
}
fn field_overridden(c: &Ctx) -> Avail {
    let Target::Field(path) = c.target else {
        return Avail::Blocked("Right-click a field".into());
    };
    let Some(entity) = c.state.selection.first() else {
        return Avail::Blocked("Select a prefab instance first".into());
    };
    match crate::shell::prefab_ops::instance(c.store, &c.state.doc, entity) {
        Some(found)
            if found.overrides.iter().any(|k| {
                k == path
                    || k.starts_with(&format!("{path}."))
                    || path.starts_with(&format!("{k}."))
            }) =>
        {
            Avail::Ready
        }
        Some(_) => Avail::Blocked("This field follows the prefab (not overridden)".into()),
        None => Avail::Blocked("Not a prefab instance".into()),
    }
}
fn prefab_source(c: &Ctx) -> Avail {
    match c.target.entity(c.state) {
        Some(entity)
            if entity.contains('/')
                || crate::shell::prefab_ops::instance(c.store, &c.state.doc, entity).is_some() =>
        {
            Avail::Blocked("Already a prefab instance; unpack it first".into())
        }
        Some(_) => Avail::Ready,
        None => Avail::Blocked("Select the entity to turn into a prefab".into()),
    }
}
fn agent_target(c: &Ctx) -> Avail {
    match c.target {
        Target::Text(agent) if c.state.presence.contains_key(agent) => Avail::Ready,
        _ => Avail::Blocked(
            "No agent has shared a view yet; agents appear when they connect over MCP".into(),
        ),
    }
}
fn agent_follow(c: &Ctx) -> Avail {
    match agent_target(c) {
        Avail::Ready => Avail::Checked(
            matches!(c.target, Target::Text(a) if c.state.follow.as_ref() == Some(a)),
        ),
        blocked => blocked,
    }
}
fn following(c: &Ctx) -> Avail {
    if c.state.follow.is_some() {
        Avail::Ready
    } else {
        Avail::Blocked("You are not following an agent".into())
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
/// The selection is a `Camera` entity (prefab-resolved entities are checked when run).
fn selected_camera(c: &Ctx) -> Avail {
    let Some(id) = c.target.entity(c.state) else {
        return Avail::Blocked("Select a Camera entity first".into());
    };
    match c
        .store
        .workspace()
        .doc(&c.state.doc)
        .and_then(|d| d.record(id))
    {
        Some(record) if !record.contains_key("Camera") => {
            Avail::Blocked("The selection has no Camera component".into())
        }
        _ => Avail::Ready,
    }
}
fn with_components(c: &Ctx, names: &[&str]) -> Avail {
    match names.iter().find(|n| c.store.schema().get(n).is_none()) {
        Some(missing) => Avail::Blocked(format!(
            "`{missing}` is not registered in this project's schema"
        )),
        None => scene_doc(c),
    }
}
fn component_target(c: &Ctx) -> Avail {
    if matches!(c.target, Target::Component(_)) && !c.state.selection.is_empty() {
        Avail::Ready
    } else {
        Avail::Blocked("Choose a component in the Inspector".into())
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
    // Native window actions share the command registry with menus, shortcuts and MCP.
    c.extend([
        cmd(
            "window.minimize",
            "Minimize Window",
            "minus",
            "",
            "Minimize the editor window.",
            |s, _, _| {
                s.window_request = Some(super::super::WindowRequest::Minimize);
                Ok(())
            },
        ),
        cmd(
            "window.maximize_window",
            "Maximize or Restore Window",
            "maximize",
            "",
            "Toggle the editor window's native maximize state.",
            |s, _, _| {
                s.window_request = Some(super::super::WindowRequest::Maximize);
                Ok(())
            },
        ),
    ]);
    // File
    c.extend([
        cmd("file.new_scene", "New Scene…", "file", "Ctrl+N", "Create an empty scene under levels/ and open it.", |s, e, t| {s.select_asset_folder(e,t)?;s.new_scene(e)}),
        later("file.new_project", "New Project…", "folder", "", "Create projects with `ara init <dir>`; the in-editor project wizard comes with the launcher."),
        cmd("file.open", "Open Scene…", "clapperboard", "Ctrl+O", "Pick a scene to open (recent first), or type to find any scene or asset.", |s, e, _| s.open_palette(e, "#")),
        later("file.open_project", "Open Project…", "folder", "", "One editor process owns one project; start `ara-editor <project>` for another."),
        cmd("file.open_scene", "Open Scene", "clapperboard", "", "Open a scene document in the Scene view.", |_, e, t| {
            let Target::Doc(doc) = t else { return Err("choose a scene".into()) };
            act(e, "editor.scene.open", json!({"scene": doc}))?;
            focus(e, "scene")
        }),
        cmd("file.save", "Save", "save", "Ctrl+S", "Write every changed document to disk now.", |s, e, _| {
            e.host.flush();
            s.status_note = Some("All changes saved".into());
            Ok(())
        }),
        later("file.save_as", "Save As…", "file", "Ctrl+Shift+S", "Documents save in place through the host journal; Save As needs a typed document-copy command."),
        cmd("file.save_all", "Save All", "save", "Ctrl+Alt+S", "Write every changed document to disk now.", |s, e, _| {
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
        cmd("build.profile.create", "Create Build Profile", "file-plus", "", "Write the default Windows profile (presets/windows.export.json): the real game runtime in a plain folder.", |s, e, _| s.create_build_profile(e).map(drop))
            .when(|c| if c.build.profile { Avail::Blocked("The project already has a build profile in presets/".into()) } else { Avail::Ready }),
        cmd("build.current", "Build Current Profile", "package", "Ctrl+Shift+B", "Export the game with the project's first profile (creating presets/windows.export.json when there is none) into builds/; progress shows in the status bar, problems in Problems.", |s, e, _| s.build_current(e)).when(|c| match &c.build {
            BuildState { running: true, .. } => Avail::Blocked("A build is running; see the status bar".into()),
            _ => Avail::Ready,
        }),
        cmd("build.run", "Run Build", "play", "", "Launch the last build: the exported game, outside the editor.", |s, e, _| s.run_build(e)).when(|c| match &c.build {
            BuildState { running: true, .. } => Avail::Blocked("A build is running; see the status bar".into()),
            BuildState { built: false, .. } => Avail::Blocked("Build the project first (Build Current Profile)".into()),
            _ => Avail::Ready,
        }),
        later("build.package", "Package…", "package", "", "Run `ara pack` for now; packaging from the editor is not built yet."),
        cmd("build.open_folder", "Open Build Folder", "folder", "", "Show the last build's folder (or builds/).", |s, e, _| {
            let folder = s.last_build.clone().unwrap_or_else(|| e.host.root().join("builds"));
            std::fs::create_dir_all(&folder)?;
            super::super::open_external(&folder)
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
        cmd("prefab.open", "Open Prefab", "package-open", "", "Edit the prefab itself in the Scene view; every instance follows.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select a prefab instance")?.to_owned();
            let found = crate::shell::prefab_ops::instance(e.host.store(), &e.state.doc, &entity).ok_or("not a prefab instance")?;
            act(e, "editor.scene.open", json!({"scene": found.prefab_doc}))
        }).when(in_instance),
        cmd("prefab.apply_all", "Apply All Overrides", "save", "", "Write this instance's overrides into the prefab, so every instance gets them.", |s, e, t| {
            let entity = t.entity(&e.state).ok_or("select a prefab instance")?.to_owned();
            s.apply_overrides(e, &entity, None)
        }).when(has_overrides),
        cmd("prefab.revert_all", "Revert All Overrides", "undo-2", "", "Drop this instance's overrides; it follows the prefab again.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select a prefab instance")?.to_owned();
            apply(e, vec![json!({"cmd": "prefab.override.revert", "args": {"entity": entity}})], "Revert overrides")
        }).when(has_overrides),
        cmd("prefab.apply_override", "Apply Override to Prefab", "save", "", "Write this field's override into the prefab, so every instance gets it.", |s, e, t| {
            let Target::Field(path) = t else { return Err("choose a field".into()) };
            let entity = e.state.selection.first().ok_or("select a prefab instance")?.clone();
            s.apply_overrides(e, &entity, Some(path))
        }).when(field_overridden),
        cmd("prefab.revert_override", "Revert Override", "undo-2", "", "Drop this field's override; it follows the prefab again.", |_, e, t| {
            let Target::Field(path) = t else { return Err("choose a field".into()) };
            let entity = e.state.selection.first().ok_or("select a prefab instance")?.clone();
            apply(e, vec![json!({"cmd": "prefab.override.revert", "args": {"entity": entity, "path": path}})], "Revert override")
        }).when(field_overridden),
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
        cmd("view.camera.fov", "Field of View…", "scan-eye", "", "Choose the Scene view's vertical field of view.", |s, _, _| { s.open_value_menu("fov"); Ok(()) })
            .labelled(|c| format!("Field of View ({})…", c.state.camera.fov_label())),
        cmd("view.camera.fov.set", "Set Field of View", "scan-eye", "", "Set the Scene view's vertical field of view: target `46` (degrees, 10–120).", |_, e, t| {
            let Target::Text(text) = t else { return Err("choose a field of view".into()) };
            let degrees: f32 = text.trim().trim_end_matches('°').trim().parse().map_err(|_| "the field of view needs a number of degrees")?;
            act(e, "editor.camera", json!({"fovYDegrees": degrees}))
        }),
        cmd("view.camera.align", "Align View to Selected Camera", "camera", "", "Look through the selected Camera entity: its position, direction and field of view.", |_, e, t| {
            let entity = t.entity(&e.state).ok_or("select a Camera entity")?.to_owned();
            act(e, "editor.camera", json!({"entity": entity}))
        }).when(selected_camera),
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
        cmd("view.camera.speed.slow", "Slow · 1 m/s", "", "", "Precise close-up camera movement.", |_, e, _| set(e,"viewport.cameraSpeed",json!(1.))).when(|c| Avail::Checked(c.state.number("viewport.cameraSpeed",4.) == 1.)),
        cmd("view.camera.speed.normal", "Normal · 4 m/s", "", "", "Default camera movement speed.", |_, e, _| set(e,"viewport.cameraSpeed",json!(4.))).when(|c| Avail::Checked(c.state.number("viewport.cameraSpeed",4.) == 4.)),
        cmd("view.camera.speed.fast", "Fast · 16 m/s", "", "", "Navigate large scenes quickly.", |_, e, _| set(e,"viewport.cameraSpeed",json!(16.))).when(|c| Avail::Checked(c.state.number("viewport.cameraSpeed",4.) == 16.)),
        later("view.camera.bookmarks", "Bookmarks", "", "", "Camera bookmarks need a per-scene editor sidecar; they are not built yet."),
        later("view.camera.pilot", "Pilot Selected Camera", "camera", "", "Piloting (moving the scene camera with the view) is not built yet; Align View to Selected Camera copies its pose and field of view."),
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
            "view.mode.unlit",
            "Unlit",
            "sun",
            "",
            "Show material base color without lighting.",
            |_, e, _| view(e, json!({"viewMode":"baseColor"})),
        )
        .when(|c| mode(c, "baseColor")),
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
        cmd(
            "view.mode.wireframe",
            "Wireframe",
            "",
            "",
            "Triangle edges over a dark background, coloured per entity.",
            |_, e, _| view(e, json!({"viewMode": "wireframe"})),
        )
        .when(|c| mode(c, "wireframe")),
        cmd(
            "view.mode.lit_wireframe",
            "Lit Wireframe",
            "",
            "",
            "The lit image with its triangle edges drawn over it.",
            |_, e, _| view(e, json!({"viewMode": "litWireframe"})),
        )
        .when(|c| mode(c, "litWireframe")),
    ]);
    c.extend([
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
        cmd("view.fidelity", "Fidelity Rendering", "sun", "", "Switch between the project's quality (ray-traced Fidelity) and the Low raster preview.", |_, e, _| {
            let low = |c| e.state.resolve_quality(c) == Some(QualityPreset::Low);
            let next = if e.state.quality { "low" } else if low(e.state.project_quality()) { "auto" } else { "project" };
            set(e, "viewport.quality", json!(next))
        }).when(|c| Avail::Checked(c.state.quality)),
        viewport_quality!("view.quality.project", "Project Default", "project", "Render the Scene view at the project's quality (Project Settings > Rendering).")
            .labelled(|c| format!("Project Default ({})", c.state.quality_label(c.state.project_quality()))),
        viewport_quality!("view.quality.auto", "Auto", "auto", "Pick the quality from this GPU: High with ray tracing (Medium under 8 GiB), Low without.")
            .labelled(|c| match c.state.detected_quality {
                Some(preset) => format!("Auto ({} on this GPU)", crate::state::preset_title(preset)),
                None => "Auto".into(),
            }),
        viewport_quality!("view.quality.low", "Low", "low", "Raster lighting with temporal antialiasing; no ray tracing."),
        viewport_quality!("view.quality.medium", "Medium", "medium", "Ray-traced lighting at half resolution, upscaled temporally; no ReSTIR."),
        viewport_quality!("view.quality.high", "High", "high", "Ray-traced lighting at two-thirds resolution; ReSTIR in scenes lit by emissives or many lights."),
        viewport_quality!("view.quality.epic", "Epic", "epic", "Native-resolution ray tracing with ReSTIR everywhere. The most expensive."),
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
        cmd("view.tool.universal", "Universal Tool", "transform", "T", "Move, rotate or scale using the combined handles.", |_, e, _| {
            view(e, json!({"gizmos": true}))?;
            act(e, "editor.tool", json!({"tool": "universal"}))
        }).when(|c| Avail::Checked(!c.state.hide_gizmos && c.state.tool == crate::state::Tool::Universal)),
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
        cmd("create.prefab", "Prefab from Selection", "package", "", "Turn the selected entity and its children into a prefab; the scene keeps a linked instance.", |s, e, t| s.prefab_from_selection(e, t)).when(prefab_source),
    ]);
    // Assets
    c.extend([
        cmd("assets.create.folder", "Folder", "folder", "", "Create a folder in the current asset folder.", |s, e, t| {s.select_asset_folder(e,t)?;s.new_folder(e)}),
        cmd("assets.create.scene", "Scene", "box", "", "Create an empty scene.", |s, e, t| {s.select_asset_folder(e,t)?;s.new_scene(e)}),
        cmd("assets.create.material", "Material", "circle", "", "Create a material graph and open it.", |s, e, t| {s.select_asset_folder(e,t)?;s.create_resource(e, "material")}),
        cmd("assets.create.vfx", "VFX System", "sparkles", "", "Create a VFX graph and open it.", |s, e, t| {s.select_asset_folder(e,t)?;s.create_resource(e, "vfx")}),
        cmd("assets.create.animation", "Animation Controller", "activity", "", "Create an animation controller and open it.", |s, e, t| {s.select_asset_folder(e,t)?;s.create_resource(e, "animation")}),
        cmd("assets.create.script", "C# Script", "file-code", "", "Create a Behaviour script in this folder; name it, compile it, then attach it.", |s, e, t| s.create_script(e,t,"behaviour",Vec::new())),
        cmd("assets.create.system", "C# System", "file-code", "", "Create a batched ECS query system from Ara's System template.", |s,e,t|s.create_script(e,t,"system",Vec::new())),
        cmd("assets.open_ide", "Open in IDE", "file-code", "", "Open the selected C# source in the configured IDE.", |s,e,t|{let path=s.asset_target(e,t).ok_or("Select a C# script")?;s.open_script_ide(e,&path)}).when(|c|match c.target {Target::Asset(p) if p.ends_with(".cs")=>Avail::Ready,_=>Avail::Blocked("Select a C# script".into())}),
        cmd("assets.create.prefab", "Prefab from Selection", "package", "", "Turn the selected entity and its children into a prefab under prefabs/.", |s, e, t| s.prefab_from_selection(e, t)).when(prefab_source),
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
        cmd("assets.show_all", "Show All Files", "file", "", "List every file, including glTF buffers, notes and project files.", |s, _, _| {
            s.assets_show_all = !s.assets_show_all;
            Ok(())
        }).when(|c| Avail::Checked(c.assets_show_all)),
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
                    // The reply is immediate; the Game view follows the start's progress.
                    params["background"] = json!(true);
                    e.embed_play(&mut params);
                    if params["embed"].is_object() {
                        act(e, "editor.layout.show", json!({"panel": "game"}))?;
                    }
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
            "Stop every player process, or cancel a Play that is still starting.",
            |_, e, _| {
                play(e, "play.stop", json!({}))?;
                view(e, json!({"play": "stopped"}))
            },
        )
        .when(|c| {
            if c.state.playing.is_some() || c.starting {
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
        cmd("tools.cs.solution", "Open Scripts in IDE", "file-code", "", "Open the project's C# module in VS Code (or the OS default editor).", |s, e, _| {
            let dir = crate::shell::scripts::modules(e.host.root()).into_iter().next().ok_or("This project has no C# module yet; use Assets > Create > C# Script")?;
            let path=dir.strip_prefix(e.host.root()).unwrap_or(&dir).to_string_lossy().replace('\\',"/");
            s.open_script_ide(e,&path)
        }).when(|_| Avail::Ready),
        later("tools.cs.reload", "Reload Scripts", "refresh-cw", "Ctrl+R", "C# reloads automatically when sources change; a manual trigger is not exposed yet."),
        cmd("window.agents", "Agents Panel", "bot", "Alt+6", "Agent activity and undo per turn.", |_, e, _| act(e, "editor.layout.show", json!({"panel": "agents"}))).when(|c| panel_open(c, "agents")),
        cmd("agents.view", "View What the Agent Sees", "eye", "", "Jump once to the agent's scene, selection and camera; your view stays yours afterwards.", |_, e, t| {
            let Target::Text(agent) = t else { return Err("choose an agent".into()) };
            act(e, "editor.follow", json!({"agent": agent}))?;
            act(e, "editor.follow", json!({}))
        }).when(agent_target),
        cmd("agents.follow", "Follow Agent", "bot", "", "Keep your view on the agent's scene, selection and camera until you stop following.", |_, e, t| {
            let Target::Text(agent) = t else { return Err("choose an agent".into()) };
            if e.state.follow.as_ref() == Some(agent) {
                act(e, "editor.follow", json!({}))
            } else {
                act(e, "editor.follow", json!({"agent": agent}))
            }
        }).when(agent_follow),
        cmd("agents.stop_following", "Stop Following", "x", "", "Return the Scene view to your own camera control.", |_, e, _| act(e, "editor.follow", json!({}))).when(following),
        cmd("agents.expand_turn", "Show Turn Changes", "chevron-down", "", "Expand or collapse what an agent turn changed.", |s, _, t| {
            if let Target::Text(turn) = t
                && !s.expanded_turns.remove(turn)
            {
                s.expanded_turns.insert(turn.clone());
            }
            Ok(())
        }),
        cmd("agents.undo_turn", "Undo Agent Turn", "undo-2", "", "Undo every change of an agent turn as new transactions; later edits by others stay.", |_, e, t| {
            let Target::Text(turn) = t else { return Err("choose an agent turn".into()) };
            // A turn is one transaction, or several sharing a `gesture-turn:<group>|` label.
            let marker = format!("gesture-turn:{turn}|");
            let txns: Vec<String> = e
                .host
                .store()
                .history()
                .iter()
                .rev()
                .filter(|x| x.id.0 == *turn || x.label.as_deref().is_some_and(|l| l.starts_with(&marker)))
                .filter(|x| e.host.store().reverted_by(&x.id).is_none())
                .map(|x| x.id.0.clone())
                .collect();
            for txn in txns {
                act(e, "review.revert", json!({"txn": txn}))?;
            }
            Ok(())
        }),
        cmd("notice.dismiss", "Dismiss Notice", "x", "", "Close this notice.", |_, e, t| {
            if let Target::Text(id) = t
                && let Ok(id) = id.parse()
            {
                e.activity.dismiss(id);
                e.panels_revision += 1;
            }
            Ok(())
        }),
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
        cmd(
            "window.game",
            "Game",
            "gamepad-2",
            "",
            "The Game view: Play inside the editor, with resolution, stats and client selection.",
            |_, e, _| act(e, "editor.layout.show", json!({"panel": "game"})),
        )
        .when(|c| panel_open(c, "game")),
        cmd(
            "game.aspect.free",
            "Free Aspect",
            "monitor",
            "",
            "Fill the Game view.",
            |s, _, _| {
                s.game_aspect = "free";
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.game.1 == "free")),
        cmd(
            "game.aspect.16x9",
            "16:9",
            "monitor",
            "",
            "Letterbox the Game view to 16:9.",
            |s, _, _| {
                s.game_aspect = "16x9";
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.game.1 == "16x9")),
        cmd(
            "game.aspect.16x10",
            "16:10",
            "monitor",
            "",
            "Letterbox the Game view to 16:10.",
            |s, _, _| {
                s.game_aspect = "16x10";
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.game.1 == "16x10")),
        cmd(
            "game.aspect.4x3",
            "4:3",
            "monitor",
            "",
            "Letterbox the Game view to 4:3.",
            |s, _, _| {
                s.game_aspect = "4x3";
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.game.1 == "4x3")),
        cmd(
            "game.aspect.21x9",
            "21:9",
            "monitor",
            "",
            "Letterbox the Game view to 21:9.",
            |s, _, _| {
                s.game_aspect = "21x9";
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.game.1 == "21x9")),
        cmd(
            "game.aspect.9x16",
            "9:16 Portrait",
            "monitor",
            "",
            "Letterbox the Game view to portrait 9:16.",
            |s, _, _| {
                s.game_aspect = "9x16";
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.game.1 == "9x16")),
        cmd(
            "game.stats",
            "Game Stats",
            "gauge",
            "",
            "Show frames per second and resolution over the Game view.",
            |s, _, _| {
                s.game_stats = !s.game_stats;
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.game.0)),
        hierarchy_filter!("hierarchy.filter.all", "All Entities", "list-tree", ""),
        hierarchy_filter!("hierarchy.filter.lights", "Lights", "lightbulb", "t:light"),
        hierarchy_filter!("hierarchy.filter.cameras", "Cameras", "camera", "t:camera"),
        hierarchy_filter!("hierarchy.filter.meshes", "Meshes", "box", "t:meshrenderer"),
        hierarchy_filter!(
            "hierarchy.filter.prefabs",
            "Prefab Instances",
            "package",
            "t:prefab"
        ),
        hierarchy_filter!("hierarchy.filter.audio", "Audio", "volume-2", "t:audio"),
        hierarchy_filter!(
            "hierarchy.filter.effects",
            "Effects",
            "sparkles",
            "t:visualeffect"
        ),
        cmd(
            "material.view_graph",
            "Material Graph",
            "layers",
            "",
            "Edit the open material as a node graph.",
            |s, _, _| {
                s.material_source_view = false;
                Ok(())
            },
        ),
        cmd(
            "material.view_source",
            "Material Source",
            "file-code",
            "",
            "Edit the open material as DSL text: the same document as the graph.",
            |s, _, _| {
                s.material_source_view = true;
                Ok(())
            },
        ),
        cmd(
            "material.node_previews",
            "Node Previews",
            "image",
            "",
            "Show or hide each material node's value preview.",
            |s, _, _| {
                s.node_previews.hidden = !s.node_previews.hidden;
                Ok(())
            },
        ),
        cmd(
            "animation.view_states",
            "Animation States",
            "person-standing",
            "",
            "Show the open controller as its state machine.",
            |s, e, _| s.animation_view(e, false),
        ),
        cmd(
            "animation.view_blends",
            "Animation Blend Spaces",
            "layers",
            "",
            "Show the open controller's pose and blend graph.",
            |s, e, _| s.animation_view(e, true),
        ),
        cmd(
            "game.client",
            "Show Client",
            "users",
            "",
            "Show and control this Play client in the Game view.",
            |s, _, t| {
                if let Target::Text(n) = t
                    && let Ok(n) = n.parse()
                {
                    s.game_client = n;
                    s.game_captured = false;
                }
                Ok(())
            },
        ),
        cmd(
            "game.capture",
            "Play in the Game View",
            "gamepad-2",
            "",
            "Send keyboard and mouse to the game until Esc.",
            |s, _, _| {
                s.game_captured = true;
                Ok(())
            },
        )
        .when(|c| {
            if c.game.2 {
                Avail::Ready
            } else {
                Avail::Blocked("Press Play first; the game shows in the Game view".into())
            }
        }),
        cmd(
            "game.release",
            "Release Game Input",
            "mouse-pointer",
            "Esc",
            "Give the keyboard and mouse back to the editor.",
            |s, _, _| {
                s.game_captured = false;
                Ok(())
            },
        ),
        cmd(
            "window.problems",
            "Problems",
            "triangle-alert",
            "Alt+5",
            "Errors and warnings grouped by file, with fixes and why.",
            |_, e, _| act(e, "editor.layout.show", json!({"panel": "problems"})),
        )
        .when(|c| panel_open(c, "problems")),
        cmd(
            "console.clear",
            "Clear Console",
            "trash",
            "",
            "Hide the messages logged so far; new ones still appear.",
            |s, e, _| {
                let logs = e.action("logs", json!({"limit": 1}))?;
                s.log_view.since = logs["cursor"].as_u64().unwrap_or(0);
                s.console_open = None;
                Ok(())
            },
        ),
        cmd(
            "console.collapse",
            "Collapse Repeated Messages",
            "layers",
            "",
            "Show one row per code with its total count.",
            |s, _, _| {
                s.log_view.collapse = !s.log_view.collapse;
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.logs.collapse)),
        cmd(
            "console.errors",
            "Show Errors",
            "circle-x",
            "",
            "Show or hide errors in the Console.",
            |s, _, _| {
                s.log_view.errors = !s.log_view.errors;
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.logs.errors)),
        cmd(
            "console.warnings",
            "Show Warnings",
            "triangle-alert",
            "",
            "Show or hide warnings in the Console.",
            |s, _, _| {
                s.log_view.warnings = !s.log_view.warnings;
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.logs.warnings)),
        cmd(
            "console.info",
            "Show Messages",
            "info",
            "",
            "Show or hide info messages in the Console.",
            |s, _, _| {
                s.log_view.info = !s.log_view.info;
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.logs.info)),
        cmd(
            "console.machine",
            "Show This Computer's Messages",
            "monitor",
            "",
            "Show or hide messages from this computer's graphics drivers and overlay layers.",
            |s, _, _| {
                s.machine_open = !s.machine_open;
                Ok(())
            },
        ),
        cmd(
            "console.select",
            "Show Message Details",
            "info",
            "",
            "Expand a Console message: explanation, subjects and fixes.",
            |s, _, t| {
                if let Target::Text(seq) = t
                    && let Ok(seq) = seq.parse()
                {
                    s.console_open = if s.console_open == Some(seq) {
                        None
                    } else {
                        Some(seq)
                    };
                }
                Ok(())
            },
        ),
        cmd(
            "problems.errors",
            "Show Error Problems",
            "circle-x",
            "",
            "Show or hide errors in Problems.",
            |s, _, _| {
                s.log_view.problem_errors = !s.log_view.problem_errors;
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.logs.problem_errors)),
        cmd(
            "problems.warnings",
            "Show Warning Problems",
            "triangle-alert",
            "",
            "Show or hide warnings in Problems.",
            |s, _, _| {
                s.log_view.problem_warnings = !s.log_view.problem_warnings;
                Ok(())
            },
        )
        .when(|c| Avail::Checked(c.logs.problem_warnings)),
        cmd(
            "problems.explain",
            "Why?",
            "lightbulb",
            "",
            "Explain a problem: what it means, common causes and fixes.",
            |s, _, t| {
                if let Target::Text(seq) = t
                    && let Ok(seq) = seq.parse()
                    && !s.explained_problems.remove(&seq)
                {
                    s.explained_problems.insert(seq);
                }
                Ok(())
            },
        ),
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
            "Open the engine documentation (README-FIRST in an installed engine).",
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
        cmd("field.set", "Set Value", "", "", "Set a field on the selection (target `Path=json`, or `Path#/pointer=json` inside a compound field).", |s, e, t| {
            let Target::Text(text) = t else { return Err("expected Path=value".into()) };
            let (path, value) = text.split_once('=').ok_or("expected Path=value")?;
            let value: Value = serde_json::from_str(value)?;
            let commands = s.field_commands(&e.state.selection, path, value)?;
            let field = path.split('#').next().unwrap_or(path);
            apply(e, commands, &format!("Set {field}"))
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
        cmd("component.reset", "Reset Component", "undo-2", "", "Restore component fields to schema or prefab defaults in one transaction.", |s, e, t| s.component_values(e, t, true)).when(component_target),
        cmd("component.remove", "Remove Component", "trash", "", "Remove the component from the selection.", |_, e, t| {
            let Target::Component(name) = t else { return Err("choose a component".into()) };
            let commands = e.state.selection.iter().map(|entity| json!({"cmd": "component.remove", "args": {"entity": entity, "component": name}})).collect();
            apply(e, commands, &format!("Remove {name}"))
        }).when(component_target),
        cmd("component.copy", "Copy Values", "copy", "", "Copy the primary selection's component values.", |s, e, t| s.copy_component(e, t)).when(component_target),
        cmd("component.paste", "Paste Values", "clipboard", "", "Paste values into this component on every selected entity.", |s, e, t| s.component_values(e, t, false)).when(|c| {
            if c.clipboard_component { Avail::Ready } else { Avail::Blocked("Copy values from this component type first".into()) }
        }),
        cmd("component.move_up", "Move Up", "chevron-up", "", "Move this section up in the Inspector.", |s, e, t| s.move_component(e, t, false)).when(|c| {
            if c.component_position.is_some_and(|(index, _)| index > 0) { Avail::Ready } else { Avail::Blocked("This is the first component".into()) }
        }),
        cmd("component.move_down", "Move Down", "chevron-down", "", "Move this section down in the Inspector.", |s, e, t| s.move_component(e, t, true)).when(|c| {
            if c.component_position.is_some_and(|(index, count)| index + 1 < count) { Avail::Ready } else { Avail::Blocked("This is the last component".into()) }
        }),
        cmd("component.new_script", "New Script…", "file-code", "", "Create a Behaviour, choose its name, then compile and attach it to the selected entities.", |s, e, t| {
            let entities = t.entities(&e.state);
            s.create_script(e,t,"behaviour",entities)
        }).when(has_entity),
        cmd("component.add", "Add Component…", "plus", "", "Search components to add, or create a script.", |s, e, _| s.toggle_add_component(e)).when(has_entity),
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
        cmd("vfx.module.toggle", "Enable or Disable Module", "check", "", "Turn the module off without removing it, or back on.", |s, e, t| vfx_module(s, e, t, "toggle")).when(vfx_module_target),
        cmd("vfx.module.delete", "Delete Module", "trash", "", "Remove the module from its stage.", |s, e, t| vfx_module(s, e, t, "delete")).when(vfx_module_target),
        cmd("vfx.restart", "Restart Effect", "rotate-ccw", "", "Run the VFX preview again from tick 0.", |s, _, _| {
            s.vfx_stream.restart();
            Ok(())
        }),
        cmd("vfx.step", "Step Effect", "step-forward", "", "Advance the paused VFX preview by one tick.", |s, e, _| {
            if e.state.preview_playing {
                act(e, "editor.workspace", json!({"playing": false}))?;
            }
            s.vfx_stream.step();
            Ok(())
        }),
        cmd("vfx.rate", "Preview Rate", "gauge", "", "Run the VFX preview faster or slower (its fixed tick is unchanged).", |s, _, t| {
            let Target::Text(rate) = t else { return Err("choose a rate".into()) };
            s.vfx_rate = rate.parse().map_err(|_| "rates are percentages")?;
            Ok(())
        }),
        cmd("vfx.emitter.collapse", "Collapse Emitter", "chevron-down", "", "Collapse or expand an emitter's stack.", |s, _, t| {
            let Target::Text(name) = t else { return Err("choose an emitter".into()) };
            if !s.vfx_collapsed.remove(name) {
                s.vfx_collapsed.insert(name.clone());
            }
            Ok(())
        }),
        cmd("vfx.emitter.hide", "Hide Emitter in Preview", "eye-off", "", "Leave an emitter out of the preview (the effect is unchanged).", |s, _, t| {
            let Target::Text(name) = t else { return Err("choose an emitter".into()) };
            if !s.vfx_hidden.remove(name) {
                s.vfx_hidden.insert(name.clone());
            }
            Ok(())
        }),
        cmd("vfx.emitter.isolate", "Isolate Emitter", "scan-eye", "", "Preview only this emitter; again to show every emitter.", |s, _, t| {
            let Target::Text(name) = t else { return Err("choose an emitter".into()) };
            s.vfx_solo = (s.vfx_solo.as_ref() != Some(name)).then(|| name.clone());
            Ok(())
        }),
        cmd("console.copy", "Copy Message", "copy", "", "Copy the diagnostic text.", |s, _, t| {
            let Target::Text(text) = t else { return Err("choose a message".into()) };
            s.copy_text(text.clone());
            Ok(())
        }),
        cmd("profiler.record", "Record", "circle-dot", "", "Start recording editor frames.", |s, e, _| s.profiler_record(e)),
        cmd("profiler.freeze", "Freeze Capture", "pause", "", "Freeze and save the current trace.", |_, e, _| act(e, "editor.profile.freeze", json!({}))),
        cmd("profiler.baseline", "Set Baseline", "flag", "", "Freeze the trace as the comparison baseline.", |_, e, _| act(e, "editor.profile.freeze", json!({"baseline": true}))),
        cmd("profiler.scope", "Outline Scope", "crosshair", "", "Outline a scope's spans in the Profiler timeline; again clears it.", |s, _, t| {
            if let Target::Text(label) = t {
                s.profile_scope = (s.profile_scope.as_deref() != Some(label.as_str())).then(|| label.clone());
            }
            Ok(())
        }),
        cmd("profiler.compare", "Compare", "git-compare", "", "Compare the frozen capture with the baseline.", |_, e, _| act(e, "editor.workspace", json!({"compare": !e.state.profiler_compare})))
            .when(|c| Avail::Checked(c.state.profiler_compare)),
        cmd("settings.close", "Close Settings", "x", "", "Close the Settings window.", |_, e, _| act(e, "editor.layout.close", json!({"panel": "settings"}))),
    ]);
    c
}
