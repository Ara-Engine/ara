//! Scene-host acceptance uses the production SDL ingress and completed native UI targets.
use super::*;
use sdl3_sys::{events::*, scancode::SDL_Scancode, video::SDL_WindowID};

#[derive(Clone, Copy, Debug)]
pub(super) enum Act {
    Prepare,
    Resize,
    Orbit,
    Pick,
    CancelMarquee,
    Main,
}

pub(super) struct Check {
    entity: Option<String>,
    camera: crate::state::Camera,
    selection: Vec<String>,
    revision: u64,
    records: Value,
    viewport: Option<Box2>,
    observations: Vec<Value>,
    input: Vec<Value>,
    waited: u32,
}
impl Default for Check {
    fn default() -> Self {
        Self {
            entity: None,
            camera: crate::state::Camera::default(),
            selection: Vec::new(),
            revision: 0,
            records: Value::Null,
            viewport: None,
            observations: Vec::new(),
            input: Vec::new(),
            waited: 0,
        }
    }
}

fn records(editor: &Editor) -> ara_ui::Result<Value> {
    let doc = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Scene docking proof document is missing")?;
    Ok(json!(doc.records()))
}

/// Inject platform events through SDL, retaining the real destination window tag.
pub(super) fn native(id: u32, event: Event, events: &mut Vec<(u32, Event)>) -> ara_ui::Result<()> {
    let signature = match &event {
        Event::MouseMotion { .. } => (0, 0, false),
        Event::MouseButton { button, pressed } => (1, i32::from(*button), *pressed),
        Event::Key {
            scancode, pressed, ..
        } => (2, *scancode, *pressed),
        _ => return Err("Scene docking proof only injects native pointer/key events".into()),
    };
    let mut raw = match event {
        Event::MouseMotion { position, delta } => SDL_Event {
            motion: SDL_MouseMotionEvent {
                r#type: SDL_EVENT_MOUSE_MOTION,
                windowID: SDL_WindowID(id),
                x: position[0],
                y: position[1],
                xrel: delta[0],
                yrel: delta[1],
                ..Default::default()
            },
        },
        Event::MouseButton { button, pressed } => SDL_Event {
            button: SDL_MouseButtonEvent {
                r#type: if pressed {
                    SDL_EVENT_MOUSE_BUTTON_DOWN
                } else {
                    SDL_EVENT_MOUSE_BUTTON_UP
                },
                windowID: SDL_WindowID(id),
                button,
                down: pressed,
                clicks: 1,
                ..Default::default()
            },
        },
        Event::Key {
            scancode, pressed, ..
        } => SDL_Event {
            key: SDL_KeyboardEvent {
                r#type: if pressed {
                    SDL_EVENT_KEY_DOWN
                } else {
                    SDL_EVENT_KEY_UP
                },
                windowID: SDL_WindowID(id),
                scancode: SDL_Scancode(scancode),
                down: pressed,
                ..Default::default()
            },
        },
        _ => return Err("Scene docking proof only injects native pointer/key events".into()),
    };
    // SAFETY: each match arm initializes the union member named by its event type.
    if !unsafe { SDL_PushEvent(&mut raw) } {
        return Err("Scene docking proof: SDL rejected the native input event".into());
    }
    let decoded = Window::poll_tagged()?;
    let delivered = decoded.iter().any(|(window, event)| {
        *window == id
            && match (signature, event) {
                ((0, _, _), Event::MouseMotion { .. }) => true,
                (
                    (1, button, pressed),
                    Event::MouseButton {
                        button: actual,
                        pressed: down,
                    },
                ) => button == i32::from(*actual) && pressed == *down,
                (
                    (2, scan, pressed),
                    Event::Key {
                        scancode,
                        pressed: down,
                        ..
                    },
                ) => scan == *scancode && pressed == *down,
                _ => false,
            }
    });
    events.extend(decoded);
    if !delivered {
        return Err(
            "Scene docking proof: SDL did not preserve the destination window/input event".into(),
        );
    }
    Ok(())
}

impl Check {
    pub fn begin(&mut self, editor: &Editor, shell: &Shell) -> ara_ui::Result<()> {
        self.camera = editor.state.camera;
        self.selection.clone_from(&editor.state.selection);
        self.revision = editor.host.store().revision();
        self.records = records(editor)?;
        self.viewport = shell.scene_host().map(|(_, rect)| rect);
        self.observations.clear();
        self.input.clear();
        self.waited = 0;
        Ok(())
    }

    fn push(
        &mut self,
        id: u32,
        event: Event,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        self.input
            .push(json!({"window":id,"event":format!("{event:?}")}));
        native(id, event, events)
    }

    fn motion(
        &mut self,
        float: &windows::Float,
        point: [f32; 2],
        delta: [f32; 2],
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        self.push(
            float.sdl,
            Event::MouseMotion {
                position: point.map(|v| v / scale),
                delta: delta.map(|v| v / scale),
            },
            events,
        )
    }

    fn button(
        &mut self,
        id: u32,
        pressed: bool,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        self.push(id, Event::MouseButton { button: 1, pressed }, events)
    }

    fn key(
        &mut self,
        id: u32,
        scancode: i32,
        pressed: bool,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        self.push(
            id,
            Event::Key {
                scancode,
                pressed,
                repeat: false,
            },
            events,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn inject(
        &mut self,
        act: Act,
        age: u32,
        editor: &mut Editor,
        shell: &mut Shell,
        floats: &windows::Floats,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if matches!(act, Act::Prepare) {
            if age != 0 {
                return Ok(());
            }
            shell.leave();
            let view = editor.view()?;
            let entity = view.entities().iter().filter(|(id, entity)| {
                let mesh = entity.record.get("MeshRenderer");
                !editor.state.hidden.contains(*id) && !editor.state.locked.contains(*id)
                    && mesh.is_some_and(|mesh| {
                        mesh.get("model").is_none_or(|value|value.is_null() || value.as_str()==Some(""))
                            && mesh.get("shape").is_none_or(|value|value.as_str()==Some("box"))
                    })
            }).min_by_key(|(_, entity)| entity.record.get("name").and_then(Value::as_str)!=Some("Salvage crate"))
                .map(|(id,_)|id.to_owned())
                .ok_or("Scene docking proof requires visible procedural box geometry; run --verify-docking --sample on a fresh scratch project")?;
            editor.action("editor.layout.show", json!({"panel":"scene"}))?;
            editor.action("editor.view", json!({"gizmos":false,"gameView":false}))?;
            editor.action("editor.select", json!({"entities":[entity]}))?;
            shell.rebuild(editor)?;
            let rect = shell
                .viewport()
                .ok_or("Scene prepare has no main viewport")?;
            editor.action("editor.frame", json!({"aspect":rect.w/rect.h}))?;
            self.entity = Some(entity);
            shell.rebuild(editor)?;
            return Ok(());
        }
        if matches!(act, Act::Main) {
            return Ok(());
        }
        shell.leave();
        let (host, rect) = shell
            .scene_host()
            .ok_or("Scene proof viewport is not visible")?;
        let id = host.ok_or("Scene proof expected the Scene in its detached window")?;
        let float = floats
            .windows
            .iter()
            .find(|float| float.id == id)
            .ok_or("Scene proof native window has not opened")?;
        let dpi = float.window.display_scale().max(0.25) * windows::ui_scale(editor);
        let scale = crate::native::points_to_logical(float.window.raw(), dpi);
        match (act, age) {
            (Act::Resize, 0) => {
                let size = [
                    float.rect[2].max(640) as u32 + 160,
                    float.rect[3].max(480) as u32 + 100,
                ];
                float.window.set_size(size)?;
                events.extend(Window::poll_tagged()?);
                self.input
                    .push(json!({"window":float.sdl,"resizePoints":size,"dpi":dpi,"scale":scale}));
            }
            (Act::Orbit, 1) => self.motion(float, centre(rect), [0.; 2], scale, events)?,
            (Act::Orbit, 2) => {
                self.key(float.sdl, 226, true, events)?;
                self.button(float.sdl, true, events)?;
            }
            (Act::Orbit, n @ 3..=5) => {
                let p = centre(rect);
                let offset = [8. * (n - 2) as f32, -4. * (n - 2) as f32];
                self.motion(
                    float,
                    [p[0] + offset[0], p[1] + offset[1]],
                    [8., -4.],
                    scale,
                    events,
                )?;
            }
            (Act::Orbit, 6) => {
                self.button(float.sdl, false, events)?;
                self.key(float.sdl, 226, false, events)?;
            }
            (Act::Pick, 0) => {
                editor.action("editor.select", json!({"entities":[]}))?;
                shell.rebuild(editor)?;
            }
            (Act::Pick, 2) => {
                let entity = self
                    .entity
                    .as_deref()
                    .ok_or("Scene proof expected entity is missing")?;
                let view = editor.view()?;
                let world = crate::world::world_transform(view.entities(), entity)
                    .ok_or("Scene proof selected entity was deleted")?;
                let point = overlay::project(
                    &editor.state.camera,
                    rect,
                    world.transform_point3(glam::Vec3::ZERO),
                )
                .filter(|p| rect.contains(p[0], p[1]))
                .ok_or("Scene proof object center is outside its hosting viewport")?;
                self.motion(float, point, [0.; 2], scale, events)?;
            }
            (Act::Pick, 3) => self.button(float.sdl, true, events)?,
            (Act::Pick, 4) => self.button(float.sdl, false, events)?,
            (Act::CancelMarquee, 1) => self.motion(
                float,
                [rect.x + rect.w * 0.1, rect.y + rect.h * 0.85],
                [0.; 2],
                scale,
                events,
            )?,
            (Act::CancelMarquee, 2) => self.button(float.sdl, true, events)?,
            (Act::CancelMarquee, n @ 3..=5) => {
                let t = (n - 2) as f32 / 3.;
                self.motion(
                    float,
                    [
                        rect.x + rect.w * (0.1 + 0.7 * t),
                        rect.y + rect.h * (0.85 - 0.6 * t),
                    ],
                    [12., -12.],
                    scale,
                    events,
                )?;
            }
            (Act::CancelMarquee, 6) => self.key(float.sdl, 41, true, events)?,
            (Act::CancelMarquee, 7) => {
                self.button(float.sdl, false, events)?;
                self.key(float.sdl, 41, false, events)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Inspect only the owning target's interior; chrome and grid strokes cannot
    /// turn a stretched black placeholder into a dense, varied scene image.
    pub fn capture(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        floats: &windows::Floats,
        target: u32,
        image: &image::RgbaImage,
        stage: &str,
    ) {
        let Some(host) = windows::resolve_scene_host(
            editor,
            shell,
            floats,
            image.width() as f32 / shell.ui.size[0].max(1.),
        ) else {
            return;
        };
        if host.target != target {
            return;
        }
        let rect = host.viewport;
        let mut colors = std::collections::BTreeSet::new();
        let mut dense = 0;
        let mut samples = 0;
        for row in 0..32 {
            for column in 0..32 {
                let x = ((rect.x + rect.w * (0.12 + 0.76 * column as f32 / 31.)) * host.dpi).floor()
                    as u32;
                let y =
                    ((rect.y + rect.h * (0.25 + 0.6 * row as f32 / 31.)) * host.dpi).floor() as u32;
                if x >= image.width() || y >= image.height() {
                    continue;
                }
                let rgba = image.get_pixel(x, y).0;
                samples += 1;
                dense +=
                    usize::from(u32::from(rgba[0]) + u32::from(rgba[1]) + u32::from(rgba[2]) > 12);
                colors.insert([rgba[0] / 4, rgba[1] / 4, rgba[2] / 4]);
            }
        }
        let window = host
            .surface
            .and_then(|id| floats.windows.iter().find(|float| float.id == id));
        let ui = host
            .surface
            .and_then(|id| shell.surface(id))
            .map_or(&shell.ui, |surface| &surface.ui);
        let registered=ui.find("viewport-image").is_some_and(|index| {
            let node=ara_ui::source::stable_id(&ui.nodes[index].element.id);
            ui.display.commands.iter().any(|draw|matches!(draw,ara_ui::paint::Draw::Image{node:draw_node,image,..} if *draw_node==node && *image==overlay::viewport_image()))
        });
        let marquee=ui.display.commands.iter().any(|draw|matches!(draw,ara_ui::paint::Draw::Rect{node,..}|ara_ui::paint::Draw::Border{node,..} if *node==ara_ui::source::stable_id("scene-marquee")));
        let non_placeholder = samples == 1024 && dense > samples / 2 && colors.len() > 16;
        if self
            .observations
            .last()
            .is_some_and(|value| value["stage"] == stage)
        {
            self.observations.pop();
        }
        self.observations.push(json!({"stage":stage,"surface":host.surface,"window":window.map(|float|float.sdl),"target":target,"dpi":host.dpi,"viewport":rect,"windowRect":window.map(|float|float.rect),"physical":image.dimensions(),"registeredSceneImage":registered,"sceneSamples":samples,"denseSceneSamples":dense,"quantizedColors":colors.len(),"sampledSceneImage":non_placeholder,"marqueeVisible":marquee,"camera":editor.state.camera,"selection":editor.state.selection,"revision":editor.host.store().revision()}));
    }

    pub fn ready(&self) -> bool {
        self.observations.last().is_some_and(|value| {
            value["stage"] == "after"
                && value["sampledSceneImage"] == true
                && value["registeredSceneImage"] == true
        })
    }

    pub fn wait_for_frame(&mut self) -> bool {
        self.waited += 1;
        self.waited < 240
    }

    pub fn result(
        &self,
        act: Option<Act>,
        editor: &Editor,
        shell: &Shell,
    ) -> ara_ui::Result<(bool, Value)> {
        let unchanged =
            editor.host.store().revision() == self.revision && records(editor)? == self.records;
        let host = shell.scene_host();
        let count = std::iter::once(&shell.ui)
            .chain(shell.floats.iter().map(|surface| &surface.ui))
            .filter(|ui| {
                ui.find("viewport-image")
                    .is_some_and(|index| ui.shown(index))
            })
            .count();
        let positive = host.is_some_and(|(_, rect)| rect.w > 1. && rect.h > 1.);
        let camera_changed = editor.state.camera != self.camera;
        let camera_orbited = camera_changed
            && editor.state.camera.target == self.camera.target
            && editor.state.camera.distance == self.camera.distance;
        let resized = self
            .viewport
            .zip(host.map(|(_, rect)| rect))
            .is_some_and(|(before, after)| after.w > before.w + 1. && after.h > before.h + 1.);
        let picked = self.entity.as_ref().is_some_and(|entity| {
            editor.state.selection.len() == 1 && editor.state.selection.first() == Some(entity)
        });
        let marquee_started = self
            .observations
            .iter()
            .any(|value| value["stage"] == "preview" && value["marqueeVisible"] == true);
        let marquee_closed = self
            .observations
            .last()
            .is_some_and(|value| value["marqueeVisible"] == false);
        let behavior = match act {
            Some(Act::Resize) => resized && !camera_changed,
            Some(Act::Orbit) => camera_orbited,
            Some(Act::Pick) => picked && !camera_changed,
            Some(Act::CancelMarquee) => {
                marquee_started
                    && marquee_closed
                    && editor.state.selection == self.selection
                    && !camera_changed
            }
            Some(Act::Main) => host.is_some_and(|(surface, _)| surface.is_none()),
            Some(Act::Prepare) | None => true,
        };
        let ok = unchanged && count == 1 && positive && self.ready() && behavior;
        Ok((
            ok,
            json!({"oneVisibleSceneHost":count==1,"positiveViewport":positive,"unchangedAuthoring":unchanged,"resized":resized,"cameraOrbited":camera_orbited,"pickedExpectedEntity":picked,"entity":self.entity,"marqueeStarted":marquee_started,"marqueeClosed":marquee_closed,"input":self.input,"observations":self.observations,"layout":editor.state.layout,"floating":editor.state.floating,"waitedFrames":self.waited}),
        ))
    }
}
