//! Inject only production mouse/key/text events for the measured actions.
use super::*;
use crate::shell::Binding;
mod cleanup;
pub(super) fn tap(window: &Window, key: i32, events: &mut Vec<(u32, Event)>) {
    for pressed in [true, false] {
        events.push((
            window.id(),
            Event::Key {
                scancode: key,
                pressed,
                repeat: false,
            },
        ));
    }
}
fn key(window: &Window, key: i32, pressed: bool, events: &mut Vec<(u32, Event)>) {
    events.push((
        window.id(),
        Event::Key {
            scancode: key,
            pressed,
            repeat: false,
        },
    ));
}
fn button(window: &Window, button: u8, pressed: bool, events: &mut Vec<(u32, Event)>) {
    events.push((window.id(), Event::MouseButton { button, pressed }));
}
fn control(shell: &Shell, id: &str) -> Option<[f32; 2]> {
    let index = shell.shown_node(id)?;
    let node = &shell.ui.nodes[index];
    let point = [
        node.rect.x + node.rect.w * 0.5,
        node.rect.y + node.rect.h * 0.5,
    ];
    (!node.state.disabled && node.clip.contains(point[0], point[1])).then_some(point)
}
fn asset_point(shell: &Shell, storage: &str) -> Option<[f32; 2]> {
    shell.ui.nodes.iter().enumerate().find_map(|(index,node)|{
        if !shell.ui.shown(index) || !matches!(shell.bindings.get(&ara_ui::source::stable_id(&node.element.id)),Some(Binding::Asset(asset)) if asset.path==storage){return None;}
        let point=[node.rect.x+node.rect.w*0.5,node.rect.y+node.rect.h*0.5];node.clip.contains(point[0],point[1]).then_some(point)
    })
}
/// Report both the mounted card and the viewport that clips it when readiness waits.
pub(super) fn source_control_probe(shell: &Shell, storage: Option<&str>) -> Value {
    let geometry = |id: &str| {
        shell.ui.find(id).map(|index| {
            let node = &shell.ui.nodes[index];
            json!({"id":id,"shown":shell.ui.shown(index),"rect":node.rect,"clip":node.clip})
        })
    };
    let card=storage.and_then(|storage|shell.ui.nodes.iter().enumerate().find_map(|(index,node)| {
        matches!(shell.bindings.get(&ara_ui::source::stable_id(&node.element.id)),Some(Binding::Asset(asset)) if asset.path==storage).then(||json!({"id":node.element.id,"shown":shell.ui.shown(index),"rect":node.rect,"clip":node.clip,"reachable":asset_point(shell,storage).is_some()}))
    }));
    json!({"source":storage,"card":card,"grid":geometry("asset-grid"),"content":geometry("assets-content")})
}
fn menu_control(shell: &Shell, command: &str) -> Option<String> {
    shell.ui.nodes.iter().enumerate().find_map(|(index,node)|{
        (shell.ui.shown(index) && node.element.attrs.get("class").is_some_and(|class|class.split_whitespace().any(|class|class=="menu-item")) && matches!(shell.bindings.get(&ara_ui::source::stable_id(&node.element.id)),Some(Binding::Command(id,_)) if *id==command)).then(||node.element.id.clone())
    })
}
fn dialog_control(shell: &Shell, suffix: &str) -> Option<String> {
    shell.ui.nodes.iter().enumerate().find_map(|(index, node)| {
        (shell.ui.shown(index)
            && node.element.id.starts_with("editor-confirm-")
            && node.element.id.ends_with(suffix))
        .then(|| node.element.id.clone())
    })
}
fn snapshot_job(report: &Value) -> Option<String> {
    report["job"]["id"].as_str().map(str::to_owned)
}
impl AssetOpsCheck {
    fn motion(
        &mut self,
        window: &Window,
        point: [f32; 2],
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) {
        events.push((
            window.id(),
            Event::MouseMotion {
                position: point.map(|value| value / scale),
                delta: [
                    (point[0] - self.pointer[0]) / scale,
                    (point[1] - self.pointer[1]) / scale,
                ],
            },
        ));
        self.pointer = point;
    }
    fn click_point(
        &mut self,
        window: &Window,
        point: [f32; 2],
        scale: f32,
        right: bool,
        events: &mut Vec<(u32, Event)>,
    ) {
        self.motion(window, point, scale, events);
        let mouse = if right { 3 } else { 1 };
        for pressed in [true, false] {
            button(window, mouse, pressed, events);
        }
    }
    fn click(
        &mut self,
        shell: &Shell,
        window: &Window,
        id: &str,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let point = control(shell, id)
            .ok_or_else(|| format!("Native Assets control {id} is not reachable or enabled"))?;
        self.click_point(window, point, scale, false, events);
        Ok(())
    }
    fn choose(
        &mut self,
        shell: &Shell,
        window: &Window,
        command: &str,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let id = menu_control(shell, command)
            .ok_or_else(|| format!("The relevant native context menu does not expose {command}"))?;
        self.click(shell, window, &id, scale, events)
    }
    fn text(
        &mut self,
        shell: &Shell,
        window: &Window,
        id: &str,
        value: &str,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        self.click(shell, window, id, scale, events)?;
        key(window, 224, true, events);
        tap(window, 4, events);
        key(window, 224, false, events);
        events.push((window.id(), Event::TextInput { text: value.into() }));
        Ok(())
    }
    fn click_asset(
        &mut self,
        shell: &Shell,
        window: &Window,
        authored: &str,
        scale: f32,
        right: bool,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let storage = fixture::storage(shell, authored)
            .ok_or_else(|| format!("Cached authored asset {authored} is missing"))?;
        let point = asset_point(shell, &storage).ok_or_else(|| {
            format!("Authored asset {authored} is clipped or not mounted in the native browser")
        })?;
        self.click_point(window, point, scale, right, events);
        Ok(())
    }
    fn history_key(window: &Window, redo: bool, events: &mut Vec<(u32, Event)>) {
        key(window, 224, true, events);
        tap(window, if redo { 28 } else { 29 }, events);
        key(window, 224, false, events);
    }
    pub(super) fn input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let storage = fixture::storage(shell, &act.path())
            .ok_or("Native asset case folder is missing from the cached index")?;
        if self.age == 0 {
            if !self.initialized {
                shell.leave();
                shell.desktop_pointer = None;
                shell.menu = None;
                shell.dialog = None;
                editor.action("editor.layout.reset", json!({}))?;
                editor.action("editor.layout.show", json!({"panel":"assets"}))?;
                editor.action("editor.focus", json!({"panel":"assets"}))?;
                editor.action("editor.workspace",json!({"folder":if act==Act::Index{""}else{storage.as_str()},"asset":"","grid":true}))?;
                editor.action("editor.search", json!({"panel":"assets","query":""}))?;
                shell.rebuild(editor)?;
                shell.input.blur(&mut shell.ui);
                self.initialized = true;
            }
            if !self.dismiss_terminal_results(shell, window, scale, events)? {
                return self.wait();
            }
            if !matches!(act, Act::Folder | Act::Index)
                && fixture::storage(shell, &act.source())
                    .and_then(|path| asset_point(shell, &path))
                    .is_none()
            {
                return self.wait();
            }
            if editor.host.unsaved() {
                return self.wait();
            }
            self.before = fixture::snapshot(editor, &storage)?;
            self.before_history = editor.host.store().history().len();
            self.before_revision = editor.host.store().revision();
            self.old_job = snapshot_job(&shell.asset_operation_report());
            return Ok(());
        }
        if act == Act::Index {
            return self.index_input(editor, shell, window, scale, events);
        }
        match self.age {
            1 if act != Act::Folder => {
                self.click_asset(shell, window, &act.source(), scale, false, events)?
            }
            2 if act.multi() => {
                key(window, 224, true, events);
                self.click_asset(
                    shell,
                    window,
                    &format!("{}/B.prefab", act.path()),
                    scale,
                    false,
                    events,
                )?;
                key(window, 224, false, events);
            }
            3 => {
                self.selection = fixture::selection(editor, shell);
                self.selection_paths = observe::selection_paths(shell, &self.selection)?;
                if matches!(act, Act::Rename | Act::RenameCancel) {
                    tap(window, 59, events);
                } else if act.drag() {
                    self.source = asset_point(
                        shell,
                        &fixture::storage(shell, &act.source()).ok_or("Drag source missing")?,
                    )
                    .ok_or("Native drag source is not mounted")?;
                    self.motion(window, self.source, scale, events);
                    button(window, 1, true, events);
                } else if act == Act::Folder {
                    self.click(shell, window, "asset-create", scale, events)?;
                } else {
                    self.click_asset(shell, window, &act.source(), scale, true, events)?;
                }
            }
            4 => {
                if matches!(act, Act::Rename | Act::RenameCancel) {
                    self.text(
                        shell,
                        window,
                        "asset-file-name",
                        "Renamed.prefab",
                        scale,
                        events,
                    )?;
                } else if act.drag() {
                    self.motion(
                        window,
                        [self.source[0] + 18., self.source[1]],
                        scale,
                        events,
                    );
                } else if matches!(act, Act::Duplicate | Act::CancelPreparation) {
                    self.choose(shell, window, "assets.duplicate", scale, events)?;
                } else if act == Act::Folder {
                    tap(window, 81, events);
                    tap(window, 79, events);
                } else {
                    self.choose(shell, window, "assets.delete", scale, events)?;
                }
            }
            5 => {
                if matches!(act, Act::Rename | Act::RenameCancel) {
                    tap(
                        window,
                        if act == Act::RenameCancel { 41 } else { 40 },
                        events,
                    );
                } else if act.drag() {
                    let destination = if act == Act::InvalidFolder {
                        act.source()
                    } else {
                        format!("{}/Destination", act.path())
                    };
                    let path = fixture::storage(shell, &destination)
                        .ok_or("Folder drop target missing from the cached index")?;
                    self.destination = asset_point(shell, &path)
                        .ok_or("Native folder drop target is not visible")?;
                    self.motion(window, self.destination, scale, events);
                } else if act == Act::Folder {
                    self.choose(shell, window, "assets.create.folder", scale, events)?;
                } else if act == Act::CancelPreparation {
                    let report = shell.asset_operation_report();
                    let id = report["job"]["id"]
                        .as_str()
                        .ok_or("Native duplicate did not start its job")?;
                    let status = editor.action("jobs.status", json!({"job":id}))?;
                    if !crate::shell::asset_import::preparation_cancellable(&status) {
                        return Err("Asset preparation completed before a native cancellation window was observed; cancellation remains unverified".into());
                    }
                    if control(shell, "asset-operation-cancel").is_none() {
                        return self.wait();
                    }
                    self.cancel_observed = Some(status);
                }
            }
            6 => {
                if act.drag() {
                    self.drag_preview = shell.content_drag_report();
                    button(window, 1, false, events);
                } else if matches!(act, Act::Delete | Act::DeleteCancel | Act::SafeDelete) {
                    let suffix = if act == Act::DeleteCancel {
                        "-cancel"
                    } else {
                        "-accept"
                    };
                    let id = dialog_control(shell, suffix)
                        .ok_or("Native destructive confirmation is missing")?;
                    self.click(shell, window, &id, scale, events)?;
                } else if act == Act::Folder {
                    let id = dialog_control(shell, "-field")
                        .ok_or("Native folder-name prompt is missing")?;
                    self.text(shell, window, &id, "Created natively", scale, events)?;
                    tap(window, 40, events);
                } else if act == Act::CancelPreparation {
                    let report = shell.asset_operation_report();
                    let id = report["job"]["id"]
                        .as_str()
                        .ok_or("Cancelable asset job disappeared")?;
                    let status = editor.action("jobs.status", json!({"job":id}))?;
                    if !crate::shell::asset_import::preparation_cancellable(&status) {
                        return Err("Asset transaction reached its commit boundary before the native Cancel click; cancellation remains unverified".into());
                    }
                    self.click(shell, window, "asset-operation-cancel", scale, events)?;
                }
            }
            7 => return self.outcome(act, editor, shell, &storage),
            8 if act.fails() => tap(window, 41, events),
            9 if act.undo() => Self::history_key(window, false, events),
            10 if act.undo() => {
                if !self.history_ready(editor, shell, 1)? {
                    return self.wait();
                }
                let undo = fixture::snapshot(editor, &storage)?;
                if undo != self.before {
                    self.snapshot_difference = fixture::difference(&self.before, &undo);
                    return Err("One native Undo did not restore the complete asset batch and its references".into());
                }
                self.undo = Some(undo);
                let selection = fixture::selection(editor, shell);
                if selection != self.selection
                    || observe::selection_paths(shell, &selection)? != self.selection_paths
                {
                    return Err("Native Undo did not restore the original multi-selection, primary asset and logical/storage keys".into());
                }
                self.undo_selection = Some(selection);
            }
            11 if act.undo() => Self::history_key(window, true, events),
            12 if act.undo() => {
                if !self.history_ready(editor, shell, 2)? {
                    return self.wait();
                }
                let actual = fixture::snapshot(editor, &storage)?;
                if self.committed.as_ref() != Some(&actual) {
                    self.snapshot_difference = fixture::difference(
                        self.committed
                            .as_ref()
                            .ok_or("Committed snapshot is missing")?,
                        &actual,
                    );
                    return Err("One native Redo did not restore the complete asset batch".into());
                }
                let selection = fixture::selection(editor, shell);
                observe::selection_paths(shell, &selection)?;
                if self.committed_selection.as_ref() != Some(&selection) {
                    return Err("Native Redo did not restore the committed multi-selection and primary asset".into());
                }
                self.redo_selection = Some(selection);
            }
            13 if act == Act::Duplicate => {
                self.click_asset(shell, window, &act.source(), scale, false, events)?
            }
            14 if act == Act::Duplicate => {
                self.unrelated_selection = Some(fixture::selection(editor, shell));
                Self::history_key(window, false, events);
            }
            15 if act == Act::Duplicate => {
                if !self.history_ready(editor, shell, 3)? {
                    return self.wait();
                }
                let actual = fixture::snapshot(editor, &storage)?;
                if actual != self.before
                    || self.unrelated_selection.as_ref() != Some(&fixture::selection(editor, shell))
                {
                    self.snapshot_difference = fixture::difference(&self.before, &actual);
                    return Err("Undo overrode a later human asset selection".into());
                }
            }
            16 if act == Act::Duplicate => Self::history_key(window, true, events),
            17 if act == Act::Duplicate => {
                if !self.history_ready(editor, shell, 4)? {
                    return self.wait();
                }
                let actual = fixture::snapshot(editor, &storage)?;
                if self.committed.as_ref() != Some(&actual)
                    || self.unrelated_selection.as_ref() != Some(&fixture::selection(editor, shell))
                {
                    self.snapshot_difference = fixture::difference(
                        self.committed
                            .as_ref()
                            .ok_or("Committed snapshot is missing")?,
                        &actual,
                    );
                    return Err("Redo overrode a later human asset selection".into());
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn index_input(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.age {
            1 => self.text(
                shell,
                window,
                "search-assets",
                &format!("{ROOT}/Index"),
                scale,
                events,
            )?,
            3 => {
                self.index_preview = observe::projection(shell);
            }
            4 => self.text(shell, window, "search-assets", "", scale, events)?,
            5 => {
                let id = format!("asset-folder-{:x}", ara_ui::source::stable_id("settings"));
                self.click(shell, window, &id, scale, events)?;
            }
            6 => {
                let point = asset_point(shell, "settings/builds")
                    .ok_or("Project settings/builds folder is not visible in the native browser")?;
                self.click_point(window, point, scale, false, events);
            }
            7 => tap(window, 40, events),
            8 if editor.state.asset_folder != "settings/builds" => return self.wait(),
            _ => {}
        }
        Ok(())
    }
}
