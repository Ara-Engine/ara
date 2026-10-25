//! Shell-side helpers behind registry commands: clipboard, quick-open, settings
//! navigation, asset targets and OS hand-offs (file manager, terminal, docs).
use super::commands::Target;
use super::*;

/// Work only the window owner can do; the app loop drains it each frame.
#[derive(Clone, Debug, PartialEq)]
pub enum WindowRequest {
    /// Toggle full screen for the main window.
    Fullscreen,
    /// Raise a floating window by id.
    Raise(String),
    /// Move every floating window onto the main window's monitor.
    Gather,
    /// Show the OS open-file dialog and import the chosen files.
    Import,
    /// Close the editor.
    Quit,
}
impl Shell {
    /// Put text on the OS clipboard at the next frame boundary.
    pub(crate) fn copy_text(&mut self, text: String) {
        self.pending_clipboard = Some(text);
    }
    /// Open the palette, optionally in a prefix mode (`@` entities and assets, `#` scenes
    /// and assets, `?` help, `>Create ` filtered commands).
    pub(crate) fn open_palette(&mut self, editor: &mut Editor, prefix: &str) -> ara_ui::Result<()> {
        self.palette_command = None;
        self.palette_active = 0;
        editor.action(
            "editor.search",
            json!({"panel": "palette", "query": prefix}),
        )?;
        editor.action("editor.palette", json!({"open": true}))?;
        Ok(())
    }
    pub(crate) fn focus_search(&mut self, editor: &mut Editor, panel: &str) -> ara_ui::Result<()> {
        editor.action("editor.layout.show", json!({"panel": panel}))?;
        self.rebuild(editor)?;
        let _ = self.input.focus(
            &mut self.ui,
            ara_ui::source::stable_id(&format!("search-{panel}")),
        );
        Ok(())
    }
    /// Open Settings on a scope (`user` or `project`) and optional category.
    pub(crate) fn open_settings(
        &mut self,
        editor: &mut Editor,
        scope: &str,
        category: &str,
    ) -> ara_ui::Result<()> {
        editor.action(
            "editor.settings.view",
            json!({"scope": scope, "category": category}),
        )?;
        let rect = self.float_rect([960, 640]);
        editor.action(
            "editor.layout.show",
            json!({"panel": "settings", "rect": rect}),
        )?;
        Ok(())
    }
    /// A centred rectangle on the main window, in desktop pixels.
    pub(crate) fn float_rect(&self, size: [i32; 2]) -> [i32; 4] {
        let [x, y, w, h] = self.main_rect;
        [
            x + (w - size[0]).max(0) / 2,
            y + (h - size[1]).max(0) / 3,
            size[0],
            size[1],
        ]
    }
    pub(crate) fn new_scene(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let root = editor.host.root().to_owned();
        let path = (1..)
            .map(|n| format!("levels/untitled-{n}.scene"))
            .find(|p| !root.join(p).exists() && editor.host.store().workspace().find(p).is_none())
            .unwrap_or_else(|| "levels/untitled.scene".into());
        let result = editor.action(
            "editor.resource.create",
            json!({"path": path, "domain": "scene"}),
        )?;
        let doc = result["doc"]
            .as_str()
            .ok_or("scene creation returned no document")?
            .to_owned();
        editor.action("editor.scene.open", json!({"scene": doc}))?;
        editor.action("editor.focus", json!({"panel": "scene"}))?;
        Ok(())
    }
    pub(crate) fn new_folder(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let base = editor.state.asset_folder.clone();
        let root = editor.host.root().to_owned();
        let path = (1..)
            .map(|n| {
                if base.is_empty() {
                    format!("New folder {n}")
                } else {
                    format!("{base}/New folder {n}")
                }
            })
            .find(|p| !root.join(p).exists())
            .unwrap_or_default();
        std::fs::create_dir_all(root.join(&path))?;
        editor.action("editor.workspace", json!({"folder": path}))?;
        Ok(())
    }
    /// Entities' resolved records, ready to paste as new entities.
    pub(crate) fn copy_entities(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?;
        let records: Vec<Value> = target
            .entities(&editor.state)
            .iter()
            .filter_map(|id| resolved.get(id))
            .map(|e| {
                let mut record = e.record.clone();
                record.retain(|k, _| ara_doc::is_component_key(k) || k == "name");
                Value::Object(record.into_iter().collect())
            })
            .collect();
        if records.is_empty() {
            return Err("select an entity to copy".into());
        }
        self.copy_text(ara_json::to_canonical_string(
            &json!({"ara.entities": records}),
        ));
        self.status_note = Some(format!("Copied {} entities", records.len()));
        self.entity_clipboard = Some(records);
        Ok(())
    }
    pub(crate) fn paste_entities(
        &mut self,
        editor: &mut Editor,
        parent: Option<String>,
    ) -> ara_ui::Result<()> {
        let records = self
            .entity_clipboard
            .clone()
            .ok_or("copy an entity first")?;
        let commands: Vec<Value> = records
            .iter()
            .map(|r| {
                let mut components = r.clone();
                let name = components
                    .as_object_mut()
                    .and_then(|m| m.remove("name"))
                    .unwrap_or_else(|| json!("Entity"));
                let mut args = json!({"name": name, "components": components});
                if let Some(parent) = &parent {
                    args["parent"] = parent.clone().into();
                }
                json!({"cmd": "entity.create", "args": args})
            })
            .collect();
        let result = editor.apply(commands, "Paste entities", None)?;
        let ids: Vec<Value> = result["outputs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| o.get("id").cloned())
            .collect();
        editor.action("editor.select", json!({"entities": ids}))?;
        Ok(())
    }
    /// The asset a command acts on: the target, else the Assets selection.
    pub(crate) fn asset_target(&self, editor: &Editor, target: &Target) -> Option<String> {
        match target {
            Target::Asset(p) | Target::Folder(p) => Some(p.clone()),
            _ => editor.state.asset_path.clone(),
        }
    }
    pub(crate) fn asset_reference(&self, editor: &Editor, path: &str) -> Option<String> {
        self.browser
            .items
            .iter()
            .find(|a| a.path == path)
            .and_then(|a| a.current_reference(editor.host.store().workspace()))
    }
    pub(crate) fn forget_thumbnail(&mut self, path: &str) {
        self.thumbnails.invalidate(path);
        self.thumbnail_slots.remove(path);
        self.thumbnail_revision += 1;
    }
    pub(crate) fn rebuild_thumbnails(&mut self) {
        self.thumbnails.invalidate_all();
        self.thumbnail_slots
            .retain(|key, _| key.starts_with("live:"));
        self.status_note = Some("Rebuilding thumbnails…".into());
    }
    pub(crate) fn open_asset(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        if let Target::Folder(folder) = target {
            editor.action("editor.workspace", json!({"folder": folder}))?;
            return Ok(());
        }
        let path = self.asset_target(editor, target).ok_or("select an asset")?;
        let workspace = editor.host.store().workspace();
        if let Some(id) = workspace.find(&path).map(ToString::to_string)
            && let Some(doc) = workspace.doc(&id)
        {
            if doc.kind == "scene" {
                editor.action("editor.scene.open", json!({"scene": id}))?;
                editor.action("editor.focus", json!({"panel": "scene"}))?;
                return Ok(());
            }
            if matches!(doc.kind.as_str(), "graph" | "animation") {
                editor.action("editor.workspace", json!({"resource": id}))?;
                return Ok(());
            }
        }
        editor.action("editor.workspace", json!({"asset": path}))?;
        editor.action("editor.focus", json!({"panel": "inspector"}))?;
        Ok(())
    }
    pub(crate) fn show_in_assets(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let path = match target {
            Target::Asset(p) => Some(p.clone()),
            _ => target.entity(&editor.state).and_then(|entity| {
                let resolved =
                    crate::world::resolve(editor.host.store(), &editor.state.doc).ok()?;
                let model = resolved
                    .get(entity)?
                    .record
                    .get("MeshRenderer")?
                    .get("model")?
                    .as_str()?
                    .to_owned();
                Some(model.trim_start_matches('@').split('#').next()?.to_owned())
            }),
        }
        .ok_or("the selection has no asset")?;
        let folder = path
            .rsplit_once('/')
            .map(|(f, _)| f.to_owned())
            .unwrap_or_default();
        editor.action("editor.layout.show", json!({"panel": "assets"}))?;
        editor.action("editor.workspace", json!({"folder": folder, "asset": path}))?;
        Ok(())
    }
    pub(crate) fn graph_visible(&self) -> bool {
        self.ui.find("graph-canvas").is_some()
    }
    pub(crate) fn frame_graph(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.edit_graph_layout(editor, "frame")
    }
    pub(crate) fn select_all_nodes(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let ids = self
            .canvas
            .as_ref()
            .map(|c| c.node_ids())
            .unwrap_or_default();
        editor.action("editor.canvas", json!({"selection": ids}))?;
        Ok(())
    }
    pub(crate) fn delete_graph_nodes(
        &mut self,
        editor: &mut Editor,
        target: &Target,
    ) -> ara_ui::Result<()> {
        let doc = self.canvas_doc.clone().ok_or("open a graph first")?;
        let nodes: Vec<String> = match target {
            Target::Node(n) => vec![n.clone()],
            _ => editor.state.canvas_selection.iter().cloned().collect(),
        };
        if nodes.is_empty() {
            return Err("select graph nodes first".into());
        }
        let commands = nodes
            .iter()
            .map(|n| json!({"cmd": "graph.node.remove", "args": {"node": n}}))
            .collect();
        editor.apply_to(&doc, commands, "Delete graph nodes")?;
        Ok(())
    }
    pub(crate) fn profiler_record(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let method = if editor.host.profiler().is_recording() {
            "profile.stop"
        } else {
            "profile.start"
        };
        editor.action(method, json!({}))?;
        Ok(())
    }
    /// The panel commands act on when no tab was targeted: the focused panel.
    pub(crate) fn active_panel(&self, editor: &Editor) -> String {
        editor.state.focus.clone()
    }
}
/// Open a path in the OS file manager.
pub(crate) fn open_external(path: &std::path::Path) -> ara_ui::Result<()> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(path).spawn()?;
    Ok(())
}
/// Open a terminal at the project root.
pub(crate) fn open_terminal(root: &std::path::Path) -> ara_ui::Result<()> {
    if cfg!(windows) {
        if std::process::Command::new("wt")
            .arg("-d")
            .arg(root)
            .spawn()
            .is_err()
        {
            std::process::Command::new("cmd")
                .args(["/c", "start", "cmd"])
                .current_dir(root)
                .spawn()?;
        }
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .args(["-a", "Terminal"])
            .arg(root)
            .spawn()?;
    } else {
        std::process::Command::new("x-terminal-emulator")
            .current_dir(root)
            .spawn()?;
    }
    Ok(())
}
/// Open the engine documentation shipped beside this build, else the project's AGENTS.md.
pub(crate) fn open_docs(editor: &Editor) -> ara_ui::Result<()> {
    let docs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    if docs.exists() {
        return open_external(&docs);
    }
    open_external(&editor.host.root().join("AGENTS.md"))
}
