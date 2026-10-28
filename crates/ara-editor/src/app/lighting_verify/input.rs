//! SDL event ingress and geometry from the actual Lighting/Inspector retained tree.
use super::*;
use ara_ui::paint::Box2;

pub(super) fn centre(rect: Box2) -> [f32; 2] {
    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]
}
fn contains(root: &ara_ui::docking::DockNode, panel: &str) -> bool {
    match root {
        ara_ui::docking::DockNode::Area { tabs, .. } => tabs.iter().any(|tab| tab == panel),
        ara_ui::docking::DockNode::Split { first, second, .. } => {
            contains(first, panel) || contains(second, panel)
        }
    }
}
fn rect(shell: &Shell, id: &str) -> Option<Box2> {
    let index = shell.shown_node(id)?;
    let node = &shell.ui.nodes[index];
    let visible = node.rect.intersection(node.clip);
    (!node.state.disabled && visible.w > 3. && visible.h >= node.rect.h - 0.5).then_some(visible)
}
fn click(
    shell: &Shell,
    id: &str,
    window: &Window,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<bool> {
    let Some(rect) = rect(shell, id) else {
        return Ok(false);
    };
    native::motion(window, centre(rect), scale, events)?;
    native::button(window, true, events)?;
    native::button(window, false, events)?;
    Ok(true)
}
fn reveal(
    shell: &Shell,
    id: &str,
    scroll: &str,
    window: &Window,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<bool> {
    if rect(shell, id).is_some() {
        return Ok(true);
    }
    let Some(index) = shell.shown_node(scroll) else {
        return Ok(false);
    };
    let node = &shell.ui.nodes[index];
    let visible = node.rect.intersection(node.clip);
    if visible.w < 4. || visible.h < 4. {
        return Ok(false);
    }
    let direction = shell.ui.find(id).map_or(-2., |i| {
        if shell.ui.nodes[i].rect.y < visible.y {
            2.
        } else {
            -2.
        }
    });
    native::motion(window, centre(visible), scale, events)?;
    native::wheel(window, direction, events)?;
    Ok(false)
}
fn blur_scene(
    shell: &Shell,
    window: &Window,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let viewport = shell
        .viewport()
        .ok_or("Lighting history needs the visible Scene to release text focus")?;
    let point = [viewport.x + viewport.w * 0.8, viewport.y + viewport.h * 0.8];
    if shell.ui.hit_test(point[0], point[1]).is_some() {
        return Err("Lighting history focus-release point is obstructed".into());
    }
    native::motion(window, point, scale, events)?;
    native::button(window, true, events)?;
    native::button(window, false, events)
}
fn right(window: &Window, down: bool, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    use sdl3_sys::{events::*, video::SDL_WindowID};
    let mut event = SDL_Event {
        button: SDL_MouseButtonEvent {
            r#type: if down {
                SDL_EVENT_MOUSE_BUTTON_DOWN
            } else {
                SDL_EVENT_MOUSE_BUTTON_UP
            },
            windowID: SDL_WindowID(window.id()),
            button: 3,
            down,
            clicks: 1,
            ..Default::default()
        },
    };
    // SAFETY: the union member matches the initialized SDL mouse-button event type.
    if !unsafe { SDL_PushEvent(&mut event) } {
        return Err("SDL rejected Lighting camera right-button input".into());
    }
    events.extend(Window::poll_tagged()?);
    Ok(())
}
fn look(
    window: &Window,
    point: [f32; 2],
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    use sdl3_sys::{events::*, video::SDL_WindowID};
    let mut event = SDL_Event {
        motion: SDL_MouseMotionEvent {
            r#type: SDL_EVENT_MOUSE_MOTION,
            windowID: SDL_WindowID(window.id()),
            x: (point[0] + 2.) / scale,
            y: point[1] / scale,
            xrel: 2. / scale,
            ..Default::default()
        },
    };
    // SAFETY: the union member matches the initialized SDL mouse-motion event type.
    if !unsafe { SDL_PushEvent(&mut event) } {
        return Err("SDL rejected Lighting camera look input".into());
    }
    events.extend(Window::poll_tagged()?);
    Ok(())
}
impl LightingCheck {
    fn setup(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        let act = ACTS[self.index];
        shell.leave();
        shell.menu = None;
        shell.dialog = None;
        editor.action("editor.palette", json!({"open":false}))?;
        editor.action("editor.search", json!({"panel":"lighting","query":""}))?;
        editor.action("editor.search", json!({"panel":"inspector","query":""}))?;
        editor.action(
            "editor.view",
            json!({"gizmos":true,"gameView":false,"hideOverlays":false,"maximized":""}),
        )?;
        if act == Act::Open {
            editor.action("editor.layout.reset", json!({}))?;
            if contains(&editor.state.layout, "lighting")
                || editor
                    .state
                    .floating
                    .iter()
                    .any(|window| contains(&window.root, "lighting"))
            {
                editor.action("editor.layout.close", json!({"panel":"lighting"}))?;
            }
        } else {
            editor.action(
                "editor.layout.move",
                json!({"panel":"lighting","slot":"left"}),
            )?;
            editor.action("editor.layout.show", json!({"panel":"scene"}))?;
            editor.action("editor.layout.show", json!({"panel":"inspector"}))?;
        }
        self.target = match act {
            Act::ShapeSphere
            | Act::ShapeBox
            | Act::Priority
            | Act::Weight
            | Act::Blend
            | Act::Scrub
            | Act::Interior
            | Act::Walk => self.ids.get("box").cloned(),
            Act::FogDensity => self.ids.get("fog").cloned(),
            _ => None,
        };
        editor.action(
            "editor.select",
            json!({"entities":self.target.iter().collect::<Vec<_>>()}),
        )?;
        if act == Act::Walk {
            editor.action("editor.camera",json!({"target":[0.,0.,0.],"yaw":0.,"pitch":0.,"distance":10.,"projection":"perspective"}))?;
            editor.action(
                "editor.settings.set",
                json!({"key":"viewport.cameraSpeed","value":4.}),
            )?;
        } else if act == Act::Cage || act.creation() {
            editor.action("editor.camera",json!({"target":[0.,0.,0.],"yaw":0.45,"pitch":0.3,"distance":17.,"projection":"perspective"}))?;
        }
        shell.rebuild(editor)?;
        self.before = proof::snapshot(editor, None)?;
        self.observations = json!({"setup":"Typed navigation/selection only; measured changes below use raw SDL","beforeTarget":self.target.as_deref().map(|id|proof::record(editor,id)).transpose()?,"beforeOtherRecords":proof::snapshot(editor,self.target.as_deref())?});
        self.pending = Some("before");
        Ok(())
    }
    pub(super) fn step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let act = ACTS[self.index];
        if self.phase == 0 {
            native::focus_lost(window, events)?;
            return self.setup(editor, shell);
        }
        if self.phase == 11 {
            blur_scene(shell, window, scale, events)?;
            native::shortcut(window, 29, events)?;
            self.pending = Some("undo");
            return Ok(());
        }
        if self.phase == 13 {
            native::shortcut(window, 28, events)?;
            self.pending = Some("redo");
            return Ok(());
        }
        if act == Act::FogWinner && self.phase == 8 {
            let winner = [self.ids.get("fog"), self.ids.get("fog2")]
                .into_iter()
                .flatten()
                .min()
                .ok_or("Native fog identities missing")?;
            let id = format!("lighting-entry-{:016x}", ara_ui::source::stable_id(winner));
            if reveal(shell, &id, "lighting-scroll", window, scale, events)?
                && click(shell, &id, window, scale, events)?
            {
                self.pending = Some("winner");
            }
            return Ok(());
        }
        if act == Act::Walk {
            return self.walk(editor, shell, window, scale, events);
        }
        if act == Act::Open {
            match self.phase {
                2 => native::shortcut(window, 14, events)?,
                3 => {
                    if rect(shell, "palette-search").is_none() {
                        return Ok(());
                    }
                    native::text(window, "window.lighting", events)?;
                }
                _ => {
                    let row = shell.command_bindings().into_iter().find(|(id, command)| {
                        id.starts_with("command-")
                            && command == "window.lighting"
                            && rect(shell, id).is_some()
                    });
                    if let Some((id, _)) = row
                        && click(shell, &id, window, scale, events)?
                    {
                        self.pending = Some("after");
                        return Ok(());
                    }
                    return Ok(());
                }
            }
            self.phase += 1;
            return Ok(());
        }
        if act == Act::Cage {
            let entity = self
                .ids
                .get("box")
                .ok_or("Native box creation did not provide an identity")?;
            let id = format!("lighting-entry-{:016x}", ara_ui::source::stable_id(entity));
            if reveal(shell, &id, "lighting-scroll", window, scale, events)?
                && click(shell, &id, window, scale, events)?
            {
                self.target = Some(entity.clone());
                self.pending = Some("after");
            }
            return Ok(());
        }
        if act.creation() || act == Act::Interior {
            let id = match act {
                Act::Global => "lighting-create-global",
                Act::Box => "lighting-create-box",
                Act::Sphere => "lighting-create-sphere",
                Act::Fog | Act::FogWinner => "lighting-create-fog",
                _ => "lighting-preset-interior",
            };
            if reveal(shell, id, "lighting-scroll", window, scale, events)?
                && click(shell, id, window, scale, events)?
            {
                self.pending = Some("after");
            }
            return Ok(());
        }
        if matches!(act, Act::ShapeSphere | Act::ShapeBox) {
            match self.phase {
                2 => {
                    if !reveal(
                        shell,
                        "enum-PostProcessVolume-shape",
                        "inspector-scroll",
                        window,
                        scale,
                        events,
                    )? || !click(shell, "enum-PostProcessVolume-shape", window, scale, events)?
                    {
                        return Ok(());
                    }
                }
                3 => {
                    native::tap(
                        window,
                        if act == Act::ShapeSphere { 77 } else { 74 },
                        events,
                    )?;
                    if act == Act::ShapeBox {
                        native::tap(window, 81, events)?;
                    }
                }
                _ => {
                    native::tap(window, 40, events)?;
                    self.pending = Some("after");
                    return Ok(());
                }
            }
            self.phase += 1;
            return Ok(());
        }
        if act == Act::Scrub {
            if self.phase == 2 {
                let id = "edit-PostProcessVolume-weight";
                if !reveal(shell, id, "inspector-scroll", window, scale, events)? {
                    return Ok(());
                }
                let rect = rect(shell, id).ok_or("Weight numeric field missing")?;
                self.scrub_start = [rect.x + rect.w - 5., rect.y + rect.h * 0.5];
                native::motion(window, self.scrub_start, scale, events)?;
                native::button(window, true, events)?;
                self.phase = 3;
            } else if self.phase == 3 {
                native::motion(
                    window,
                    [self.scrub_start[0] + 24., self.scrub_start[1]],
                    scale,
                    events,
                )?;
                self.pending = Some("preview");
            } else {
                native::button(window, false, events)?;
                self.pending = Some("after");
            }
            return Ok(());
        }
        if let Some((field, value)) = act.field() {
            let id = format!("edit-{field}");
            if self.phase == 2 {
                if !reveal(shell, &id, "inspector-scroll", window, scale, events)?
                    || !click(shell, &id, window, scale, events)?
                {
                    return Ok(());
                }
                native::shortcut(window, 4, events)?;
                native::text(window, value, events)?;
                self.phase = 3;
            } else {
                native::tap(window, 40, events)?;
                self.pending = Some("after");
            }
        }
        Ok(())
    }
    fn walk(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let point = centre(
            shell
                .viewport()
                .ok_or("Native blend walk needs a visible Scene")?,
        );
        match self.phase {
            2 => {
                self.pending = Some("outside");
            }
            3 | 7 => {
                native::motion(window, point, scale, events)?;
                right(window, true, events)?;
                self.phase += 1;
            }
            4 | 8 => {
                look(window, point, scale, events)?;
                native::key(window, 26, true, events)?;
                self.phase += 1;
            }
            5 | 9 => {
                let threshold = if self.phase == 5 { 6.5 } else { 4.5 };
                if editor.state.camera.position().z <= threshold {
                    native::key(window, 26, false, events)?;
                    right(window, false, events)?;
                    self.pending = Some(if self.phase == 5 { "blend" } else { "inside" });
                }
            }
            _ => {}
        }
        Ok(())
    }
}
