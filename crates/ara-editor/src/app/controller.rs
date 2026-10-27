//! Native camera, picking, transform and keyboard input, independent of presentation.
use super::*;
use glam::Vec2;
use std::collections::BTreeSet;
#[cfg(test)]
mod navigation_tests;
mod selection;
#[cfg(test)]
mod selection_gesture_tests;
mod snap;
struct GizmoDrag {
    handle: overlay::Handle,
    start: [f32; 2],
    vector: Vec2,
    length: f32,
    rotation: Option<(Vec3, Vec3, Vec3)>,
    /// Plane (or view-plane) drags: normal and the pointer's starting point on it.
    plane: Option<(Vec3, Vec3)>,
    axes: [Vec3; 3],
    pivot: Vec3,
    /// Digits typed during the drag; they set the value exactly.
    typed: String,
    duplicate: bool,
    /// Captured source geometry and contact identity for exact placement modes.
    snap: Option<snap::Drag>,
}
#[derive(Default)]
pub(super) struct Controller {
    buttons: [bool; 4],
    /// The right button moved the camera, so its release opens no context menu.
    orbited: bool,
    /// Fly navigation starts only from an unobstructed Scene viewport press.
    fly_capture: bool,
    keys: BTreeSet<i32>,
    gizmo: Option<GizmoDrag>,
    /// Gizmo handle under the pointer, highlighted yellow.
    hover: Option<overlay::Handle>,
    marquee: Option<selection::Press>,
}
impl Controller {
    /// Whether any mouse button is held (window moves persist after release).
    pub fn buttons_down(&self) -> bool {
        self.buttons.iter().any(|b| *b)
    }
    /// Forget a button whose release another window consumed.
    pub fn release(&mut self, button: usize) {
        if let Some(b) = self.buttons.get_mut(button) {
            *b = false;
        }
    }
    /// Hovered and dragged gizmo handles, for drawing.
    pub fn gizmo_state(&self) -> overlay::GizmoState {
        overlay::GizmoState {
            hover: self.hover,
            active: self.gizmo.as_ref().map(|g| g.handle),
        }
    }
    /// Start a handle drag at the pointer.
    #[allow(clippy::too_many_arguments)]
    fn begin_gizmo(
        &mut self,
        editor: &mut Editor,
        handles: &overlay::Handles,
        handle: overlay::Handle,
        pointer: [f32; 2],
        rect: ara_ui::paint::Box2,
        duplicate: bool,
    ) -> ara_ui::Result<()> {
        let tool = handle.tool(editor.state.tool);
        let operation = handles.for_tool(tool);
        let handles = &operation;
        editor.begin_gesture_tool(tool)?;
        let camera = editor.state.camera;
        let forward = (camera.target - camera.position()).normalize_or_zero();
        let (vector, rotation, plane) = match handle.base() {
            overlay::Handle::Axis(axis) => (
                Vec2::from(handles.ends[axis]) - Vec2::from(handles.origin),
                (tool == crate::state::Tool::Rotate)
                    .then(|| {
                        overlay::plane_point(
                            &camera,
                            rect,
                            pointer,
                            handles.pivot,
                            handles.axes[axis],
                        )
                        .map(|start| (handles.pivot, handles.axes[axis], start))
                    })
                    .flatten(),
                None,
            ),
            overlay::Handle::Plane(axis) => (
                Vec2::ONE,
                None,
                overlay::plane_point(&camera, rect, pointer, handles.pivot, handles.axes[axis])
                    .map(|start| (handles.axes[axis], start)),
            ),
            overlay::Handle::Centre => (
                Vec2::new(1., -1.),
                None,
                overlay::plane_point(&camera, rect, pointer, handles.pivot, forward)
                    .map(|start| (forward, start)),
            ),
            overlay::Handle::RotateAxis(_) | overlay::Handle::ScaleAxis(_) => {
                unreachable!("base handle")
            }
        };
        self.gizmo = Some(GizmoDrag {
            handle,
            start: pointer,
            vector,
            length: handles.length,
            rotation,
            plane,
            axes: handles.axes,
            pivot: handles.pivot,
            typed: String::new(),
            duplicate,
            snap: None,
        });
        Ok(())
    }
    /// Apply the pointer (or typed value) to the gesture; returns the readout text.
    fn drive_gizmo(
        &self,
        editor: &mut Editor,
        position: [f32; 2],
        rect: Option<ara_ui::paint::Box2>,
    ) -> Option<String> {
        let drag = self.gizmo.as_ref()?;
        let camera = editor.state.camera;
        let snap = if editor.state.snapping {
            match drag.handle.tool(editor.state.tool) {
                crate::state::Tool::Translate | crate::state::Tool::Universal => {
                    format!(" · snap {}", editor.state.move_snap)
                }
                crate::state::Tool::Rotate => format!(" · snap {}°", editor.state.rotate_snap),
                crate::state::Tool::Scale => format!(" · snap {}", editor.state.scale_snap),
            }
        } else {
            String::new()
        };
        let gesture = editor.gesture.as_mut()?;
        let typed = drag
            .typed
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite());
        let names = ["X", "Y", "Z"];
        let (delta, text) = match drag.handle.base() {
            overlay::Handle::Axis(axis) => {
                let movement = Vec2::from(position) - Vec2::from(drag.start);
                let amount = movement.dot(drag.vector) / drag.vector.length_squared().max(0.01);
                let amount = match gesture.tool {
                    crate::state::Tool::Translate | crate::state::Tool::Universal => {
                        typed.unwrap_or(amount * drag.length)
                    }
                    crate::state::Tool::Rotate => typed.unwrap_or_else(|| {
                        if let (Some((pivot, normal, start)), Some(rect)) = (drag.rotation, rect) {
                            overlay::plane_point(&camera, rect, position, pivot, normal)
                                .map(|point| {
                                    let a = (start - pivot).normalize_or_zero();
                                    let b = (point - pivot).normalize_or_zero();
                                    normal.dot(a.cross(b)).atan2(a.dot(b)).to_degrees()
                                })
                                .unwrap_or(0.)
                        } else {
                            amount * 90.
                        }
                    }),
                    crate::state::Tool::Scale => typed.map(|v| v - 1.).unwrap_or(amount),
                };
                let mut delta = Vec3::ZERO;
                delta[axis] = amount;
                let value = match gesture.tool {
                    crate::state::Tool::Translate | crate::state::Tool::Universal => {
                        format!("{} {:+.3} m", names[axis], amount)
                    }
                    crate::state::Tool::Rotate => format!("{} {:+.1}°", names[axis], amount),
                    crate::state::Tool::Scale => format!("{} ×{:.3}", names[axis], 1. + amount),
                };
                (delta, value)
            }
            overlay::Handle::Plane(_) | overlay::Handle::Centre
                if gesture.tool == crate::state::Tool::Translate =>
            {
                let world = drag
                    .plane
                    .zip(rect)
                    .and_then(|((normal, start), rect)| {
                        overlay::plane_point(&camera, rect, position, drag.pivot, normal)
                            .map(|p| p - start)
                    })
                    .unwrap_or(Vec3::ZERO);
                let mut delta = Vec3::new(
                    world.dot(drag.axes[0]),
                    world.dot(drag.axes[1]),
                    world.dot(drag.axes[2]),
                );
                if let overlay::Handle::Plane(axis) = drag.handle {
                    delta[axis] = 0.;
                }
                (
                    delta,
                    format!("Δ {:.3}, {:.3}, {:.3} m", delta.x, delta.y, delta.z),
                )
            }
            _ => {
                // Centre of the scale tool: uniform scale.
                let movement = Vec2::from(position) - Vec2::from(drag.start);
                let amount = typed
                    .map(|v| v - 1.)
                    .unwrap_or((movement.x - movement.y) / 100.);
                (Vec3::splat(amount), format!("×{:.3}", 1. + amount))
            }
        };
        gesture.update(delta);
        let typed = if drag.typed.is_empty() {
            String::new()
        } else {
            format!("  [{}|]", drag.typed)
        };
        Some(format!("{text}{typed}{snap}"))
    }
    #[allow(clippy::too_many_arguments)]
    pub fn event(
        &mut self,
        event: Event,
        editor: &mut Editor,
        shell: &mut Shell,
        renderer: &mut Renderer,
        view: &crate::world::DocumentView,
        scene_view: &viewport::SceneView,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<(bool, bool, bool)> {
        if matches!(event, Event::FocusLost) {
            shell.native_focus_lost(clipboard)?;
            self.keys.clear();
            self.buttons = [false; 4];
            self.fly_capture = false;
            self.gizmo = None;
            self.marquee = None;
            shell.set_marquee(None);
            shell.drag = None;
            shell.drop_preview = None;
            shell.finish_scrub(editor, true)?;
            editor.finish_gesture(true)?;
            editor.end_field_gestures()?;
            shell.show_readout(None);
            return Ok((true, true, false));
        }
        match &event {
            Event::TextInput { text } => {
                let changed = shell.native_text(text, clipboard)?;
                shell.dispatch(editor)?;
                return Ok((changed, false, false));
            }
            Event::TextEditing {
                text,
                start,
                length,
            } => {
                let changed = shell.native_preedit(text, *start, *length, clipboard)?;
                return Ok((changed, false, false));
            }
            _ => {}
        }
        if shell.modal_active() || shell.choice_open() {
            if self.gizmo.take().is_some() {
                editor.finish_gesture(true)?;
                shell.show_readout(None);
            }
            self.keys.clear();
            self.buttons = [false; 4];
            self.marquee = None;
            shell.set_marquee(None);
            match event {
                Event::MouseMotion { position, .. } => {
                    shell.pointer = position;
                    shell.event(InputEvent::PointerMove(position), clipboard)?;
                }
                Event::MouseButton { button: 1, pressed } => {
                    shell.event(
                        if pressed {
                            InputEvent::PointerDown(shell.pointer)
                        } else {
                            InputEvent::PointerUp(shell.pointer)
                        },
                        clipboard,
                    )?;
                    shell.dispatch(editor)?;
                }
                Event::Wheel(delta) => {
                    shell.event(
                        InputEvent::Wheel {
                            position: shell.pointer,
                            delta: delta.map(|v| -v * 28.),
                        },
                        clipboard,
                    )?;
                }
                Event::Key {
                    scancode, pressed, ..
                } => {
                    shell.key(editor, scancode, pressed, clipboard)?;
                }
                Event::Quit => return Ok((true, false, shell.request_quit(editor)?)),
                _ => {}
            }
            return Ok((true, false, false));
        }
        if shell.graph_event(editor, &event)? {
            return Ok((true, false, false));
        }
        let mut dirty = false;
        let mut scene_dirty = false;
        let mut quit = false;
        match event {
            Event::Quit => quit = true,
            Event::Resize(_) => dirty = true,
            Event::FocusLost => {} // Lifecycle cancellation is handled before modal/viewport routing.
            Event::FileDrop { .. } => {} // The window owner dispatches the typed import.
            Event::TextInput { .. } | Event::TextEditing { .. } => {}
            Event::Key {
                scancode,
                pressed,
                repeat: _,
            } => {
                if pressed {
                    self.keys.insert(scancode);
                } else {
                    self.keys.remove(&scancode);
                }
                if pressed && scancode == 41 && self.marquee.take().is_some() {
                    shell.set_marquee(None);
                    return Ok((true, false, false));
                }
                if pressed
                    && self.gizmo.is_some()
                    && shell.modifiers[1]
                    && matches!(scancode, 28 | 29)
                {
                    self.gizmo = None;
                    editor.finish_gesture(true)?;
                    shell.show_readout(None);
                    shell.key(editor, scancode, true, clipboard)?;
                    return Ok((true, true, false));
                }
                if pressed && self.gizmo.is_some() && scancode != 41 {
                    // Typed entry during a drag: digits set the value, X/Y/Z switch axis, Enter commits.
                    let mut commit = false;
                    if let Some(drag) = &mut self.gizmo {
                        match scancode {
                            30..=38 => drag.typed.push(char::from(b'1' + (scancode - 30) as u8)),
                            39 | 98 => drag.typed.push('0'),
                            89..=97 => drag.typed.push(char::from(b'1' + (scancode - 89) as u8)),
                            55 | 99 => drag.typed.push('.'),
                            45 | 86 => drag.typed.push('-'),
                            42 => {
                                drag.typed.pop();
                            }
                            27..=29 => {
                                let axis = (scancode - 27) as usize;
                                if let overlay::Handle::Axis(_) = drag.handle.base()
                                    && let Some(rect) = shell.viewport()
                                    && let Some(handles) =
                                        overlay::handles(view, &editor.state, rect)
                                {
                                    let tool = drag.handle.tool(editor.state.tool);
                                    let handles = handles.for_tool(tool);
                                    drag.handle = match drag.handle {
                                        overlay::Handle::RotateAxis(_) => {
                                            overlay::Handle::RotateAxis(axis)
                                        }
                                        overlay::Handle::ScaleAxis(_) => {
                                            overlay::Handle::ScaleAxis(axis)
                                        }
                                        _ => overlay::Handle::Axis(axis),
                                    };
                                    drag.vector =
                                        Vec2::from(handles.ends[axis]) - Vec2::from(handles.origin);
                                    drag.rotation = (tool == crate::state::Tool::Rotate)
                                        .then(|| {
                                            overlay::plane_point(
                                                &editor.state.camera,
                                                rect,
                                                drag.start,
                                                handles.pivot,
                                                handles.axes[axis],
                                            )
                                            .map(|s| (handles.pivot, handles.axes[axis], s))
                                        })
                                        .flatten();
                                }
                            }
                            40 | 88 => commit = true,
                            _ => {}
                        }
                    }
                    if commit {
                        self.finish_snap(editor, shell)?;
                        shell.show_readout(None);
                    } else {
                        let text = self.drive_snap(editor, shell, view, scene_view);
                        shell.show_readout(text.map(|t| (t, shell.pointer)));
                    }
                    return Ok((true, true, false));
                }
                if !self.buttons[3] || !self.fly_capture {
                    shell.key(editor, scancode, pressed, clipboard)?;
                    if scancode == 41 && pressed {
                        self.gizmo = None;
                        shell.show_readout(None);
                        scene_dirty = true;
                    }
                } else {
                    match scancode {
                        225 | 229 => shell.modifiers[0] = pressed,
                        224 | 228 => shell.modifiers[1] = pressed,
                        226 | 230 => shell.modifiers[2] = pressed,
                        _ => {}
                    }
                }
                dirty = true;
            }
            Event::MouseMotion { position, delta } => {
                shell.pointer = position;
                editor.input_active();
                if self.move_marquee(shell) {
                    return Ok((true, false, false));
                }
                if shell.move_picker()? || shell.move_scrub()? {
                    return Ok((true, false, false));
                }
                let inside = shell
                    .viewport()
                    .is_some_and(|r| r.contains(position[0], position[1]))
                    && shell
                        .ui
                        .hit_test(shell.pointer[0], shell.pointer[1])
                        .is_none()
                    && !editor.state.palette
                    && shell.menu.is_none();
                if self.gizmo.is_some() {
                    if self.gizmo.as_ref().is_some_and(|g| g.typed.is_empty()) {
                        let text = self.drive_snap(editor, shell, view, scene_view);
                        shell.show_readout(text.map(|t| (t, position)));
                    }
                    scene_dirty = true;
                } else if inside
                    && (self.buttons[2]
                        || (self.buttons[3] && self.fly_capture)
                        || (self.buttons[1] && shell.modifiers[2]))
                {
                    let mut camera = editor.state.camera;
                    if self.buttons[3] && self.fly_capture && delta[0].abs() + delta[1].abs() > 0.5
                    {
                        self.orbited = true;
                    }
                    if self.buttons[2] {
                        camera.pan(delta);
                    } else if self.buttons[3] && self.fly_capture {
                        camera.look(delta);
                    } else {
                        camera.orbit(delta);
                    }
                    set_camera(editor, &camera)?;
                    scene_dirty = true;
                } else {
                    let hover = if inside {
                        shell.viewport().and_then(|rect| {
                            overlay::handles(view, &editor.state, rect)
                                .and_then(|h| overlay::hit(&h, position))
                        })
                    } else {
                        None
                    };
                    // Handle highlights are UI overlays: a redraw, not a Scene frame.
                    if hover != self.hover {
                        self.hover = hover;
                    }
                    shell.event(InputEvent::PointerMove(position), clipboard)?;
                    shell.hover_tooltip()?;
                    if shell.menu.is_some() {
                        shell.menu_hover(editor)?;
                    }
                    if shell.dragged_tab().is_some() {
                        shell.drag_over();
                    }
                    shell.drag_column(editor)?;
                }
                dirty = true;
            }
            Event::MouseButton { button, pressed } => {
                if pressed && button != 1 {
                    self.marquee = None;
                    shell.set_marquee(None);
                }
                if button < 4 {
                    self.buttons[button as usize] = pressed;
                }
                editor.input_active();
                // The colour picker's eyedropper owns clicks while it picks: a left
                // click (or releasing a drag from its button) picks, a right click cancels.
                if shell.eyedropper_active() {
                    match (button, pressed) {
                        (1, true) if !shell.eyedropper_held() => shell.pick_eyedropper(editor)?,
                        (1, false) => {
                            shell.finish_picker(editor)?;
                            shell.event(InputEvent::Cancel, clipboard)?;
                        }
                        (3, true) => {
                            shell.cancel_eyedropper();
                            shell.rebuild(editor)?;
                        }
                        _ => {}
                    }
                    return Ok((true, true, false));
                }
                if pressed
                    && shell
                        .viewport()
                        .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1]))
                    && shell
                        .ui
                        .hit_test(shell.pointer[0], shell.pointer[1])
                        .is_none()
                    && !editor.state.palette
                    && shell.menu.is_none()
                {
                    shell.commit_fields(editor)?;
                    shell.input.blur(&mut shell.ui);
                }
                // A press outside the open popovers (menus, the palette, pickers) closes
                // them; another menu title then opens its own, and the open menu's title
                // closes it when released.
                if pressed && shell.dismiss_popovers(editor, shell.pointer)? {
                    dirty = true;
                }
                if button == 3 && !pressed && !self.orbited {
                    let in_viewport = shell
                        .viewport()
                        .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1]))
                        && shell
                            .ui
                            .hit_test(shell.pointer[0], shell.pointer[1])
                            .is_none();
                    if in_viewport {
                        let rect = shell.viewport().ok_or("viewport missing")?;
                        let x = ((shell.pointer[0] - rect.x) / rect.w * renderer.size()[0] as f32)
                            .max(0.) as u32;
                        let y = ((shell.pointer[1] - rect.y) / rect.h * renderer.size()[1] as f32)
                            .max(0.) as u32;
                        let id = if x < renderer.size()[0] && y < renderer.size()[1] {
                            renderer.entity_at([x, y])?
                        } else {
                            None
                        };
                        // Through the channel map: a terrain tile is its Terrain, a
                        // model part its entity.
                        let entity = id
                            .and_then(|id| shell.outlines.channels.target_of(id))
                            .map(str::to_owned);
                        let source = match entity {
                            Some(entity) => {
                                if !editor.state.selection.contains(&entity) {
                                    editor.action("editor.select", json!({"entities":[entity]}))?;
                                }
                                crate::shell::commands::menus::Context::Entity
                            }
                            None => crate::shell::commands::menus::Context::Viewport,
                        };
                        let target = editor
                            .state
                            .selection
                            .first()
                            .cloned()
                            .filter(|_| source == crate::shell::commands::menus::Context::Entity)
                            .map(crate::shell::commands::Target::Entity)
                            .unwrap_or_default();
                        shell.open_viewport_menu(editor, source, target)?;
                    } else {
                        shell.open_context(editor)?;
                    }
                    dirty = true;
                }
                if button == 3 && pressed {
                    self.orbited = false;
                    self.fly_capture = shell
                        .viewport()
                        .is_some_and(|rect| rect.contains(shell.pointer[0], shell.pointer[1]))
                        && shell
                            .ui
                            .hit_test(shell.pointer[0], shell.pointer[1])
                            .is_none()
                        && shell.menu.is_none()
                        && !shell.modal_active();
                    if self.fly_capture {
                        editor.action("editor.focus", json!({"panel":"scene"}))?;
                    }
                }
                if button == 3 && !pressed {
                    self.fly_capture = false;
                }
                if button == 1 {
                    let inside = shell
                        .viewport()
                        .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1]))
                        && shell
                            .ui
                            .hit_test(shell.pointer[0], shell.pointer[1])
                            .is_none()
                        && !editor.state.palette
                        && shell.menu.is_none();
                    if pressed {
                        if !inside {
                            shell.commit_fields(editor)?;
                            shell.event(InputEvent::PointerDown(shell.pointer), clipboard)?;
                            shell.begin_native_scrub(editor)?;
                            if !shell.input.scrolling()
                                && !shell.begin_picker_drag(editor)?
                                && !shell.begin_scrub(editor)?
                                && !shell.begin_column_drag()
                            {
                                shell.begin_drag();
                            }
                        } else {
                            shell.commit_fields(editor)?;
                            shell.input.blur(&mut shell.ui);
                            let rect = shell.viewport().ok_or("viewport missing")?;
                            if self.begin_vertex(editor, shell, view, scene_view)? {
                                scene_dirty = true;
                            } else if let Some(handles) =
                                overlay::handles(view, &editor.state, rect)
                                && let Some(handle) = overlay::hit(&handles, shell.pointer)
                            {
                                let pointer = shell.pointer;
                                self.begin_gizmo(
                                    editor,
                                    &handles,
                                    handle,
                                    pointer,
                                    rect,
                                    shell.modifiers[2],
                                )?;
                                self.capture_snap(editor, shell, view, scene_view);
                            } else if !shell.modifiers[2] {
                                let x = ((shell.pointer[0] - rect.x) / rect.w
                                    * renderer.size()[0] as f32)
                                    .max(0.) as u32;
                                let y = ((shell.pointer[1] - rect.y) / rect.h
                                    * renderer.size()[1] as f32)
                                    .max(0.) as u32;
                                if x < renderer.size()[0] && y < renderer.size()[1] {
                                    let id = renderer.entity_at([x, y])?;
                                    // Through the channel map: a terrain tile selects its
                                    // Terrain, a model part its entity.
                                    let entity = id
                                        .and_then(|id| shell.outlines.channels.target_of(id))
                                        .map(str::to_owned)
                                        .filter(|a| !editor.state.locked.contains(a));
                                    if let Some(path) = shell.pick_field.take() {
                                        if let Some(entity) = entity {
                                            editor.apply(
                                                shell.field_commands(
                                                    editor,
                                                    &editor.state.selection,
                                                    &path,
                                                    json!(entity),
                                                )?,
                                                "Pick entity reference",
                                                None,
                                            )?;
                                        }
                                    } else {
                                        self.begin_marquee(editor, shell, entity);
                                    }
                                }
                            }
                        }
                    } else if !pressed {
                        shell.show_readout(None);
                        if self.finish_marquee(editor, shell, scene_view)? {
                            return Ok((true, false, false));
                        }
                        let native_scrub = shell.input.scrubbing();
                        if shell.finish_picker(editor)? || shell.finish_scrub(editor, false)? {
                            shell.event(InputEvent::Cancel, clipboard)?;
                        } else if self.gizmo.is_some() {
                            self.finish_snap(editor, shell)?;
                            shell.show_readout(None);
                            scene_dirty = true;
                        } else if !shell.drop_at(editor)? {
                            let slider = shell
                                .input
                                .captured()
                                .and_then(|id| shell.ui.widgets.get(&id))
                                .is_some_and(|w| {
                                    matches!(w, ara_ui::widgets::Widget::Slider { .. })
                                });
                            shell.event(InputEvent::PointerUp(shell.pointer), clipboard)?;
                            shell.dispatch(editor)?;
                            if slider {
                                shell.commit_fields(editor)?;
                            }
                        } else {
                            shell.event(InputEvent::Cancel, clipboard)?;
                        }
                        if native_scrub {
                            editor.end_field_gestures()?;
                        }
                    }
                    dirty = true;
                }
            }
            Event::Wheel(delta) => {
                self.marquee = None;
                shell.set_marquee(None);
                editor.input_active();
                if shell
                    .viewport()
                    .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1]))
                    && shell
                        .ui
                        .hit_test(shell.pointer[0], shell.pointer[1])
                        .is_none()
                    && shell.ui.scrollbar_at(shell.pointer).is_none()
                    && shell.menu.is_none()
                    && !editor.state.palette
                {
                    let mut camera = editor.state.camera;
                    camera.zoom(delta[1]);
                    set_camera(editor, &camera)?;
                    scene_dirty = true;
                } else {
                    // Keep the old card pool scrollable until its owner adopts <scroll>.
                    let legacy_grid = !editor.state.palette
                        && shell.menu.is_none()
                        && shell.ui.find("asset-grid").is_some_and(|i| {
                            shell.ui.shown(i)
                                && shell.ui.scroll_position(i).is_none()
                                && shell.ui.nodes[i]
                                    .rect
                                    .contains(shell.pointer[0], shell.pointer[1])
                        });
                    if legacy_grid {
                        shell.scroll_hierarchy(-delta[1] * 26.)?;
                    } else {
                        shell.event(
                            InputEvent::Wheel {
                                position: shell.pointer,
                                delta: [-delta[0] * 26., -delta[1] * 26.],
                            },
                            clipboard,
                        )?;
                    }
                }
                dirty = true;
            }
        }
        Ok((dirty, scene_dirty, quit))
    }
    pub fn fly(&mut self, editor: &mut Editor, shell: &Shell, dt: f32) -> ara_ui::Result<bool> {
        let mut dirty = false;
        let mut scene_dirty = false;
        if self.buttons[3]
            && self.fly_capture
            && !shell.modal_active()
            && editor.state.focus == "scene"
            && dt.is_finite()
            && dt > 0.
        {
            let direction = Vec3::new(
                (i32::from(self.keys.contains(&7)) - i32::from(self.keys.contains(&4))) as f32,
                (i32::from(self.keys.contains(&8)) - i32::from(self.keys.contains(&20))) as f32,
                (i32::from(self.keys.contains(&22)) - i32::from(self.keys.contains(&26))) as f32,
            );
            if direction.length_squared() > 0. {
                let mut camera = editor.state.camera;
                let speed = editor
                    .state
                    .settings
                    .get("viewport.cameraSpeed")
                    .and_then(Value::as_f64)
                    .filter(|value| value.is_finite())
                    .unwrap_or(4.)
                    .clamp(0.5, 50.) as f32;
                camera.fly(
                    direction.normalize(),
                    dt * speed * if shell.modifiers[0] { 3. } else { 1. },
                );
                set_camera(editor, &camera)?;
                scene_dirty = true;
                dirty = true;
            }
        }

        Ok(dirty || scene_dirty)
    }
}
