//! A UI-only native modal host; no project, scripts or scene renderer is opened.
use super::{
    State,
    view::{self, Action},
};
use ara_editor_gpu::{NativeUi, UiDevice};
use ara_platform::{Event, Present, Window};
use ara_ui::{
    Ui,
    input::{InputEvent, InputState},
    layers::{LayerKind, LayerStack},
    source::stable_id,
};
use ara_ui_render::{atlas::TextMode, gpu::Renderer};
use ara_ui_text::TextSystem;

pub(super) struct DialogWindow {
    adapter: NativeUi,
    present: Option<Present>,
    device: UiDevice,
    window: Window,
    renderer: Renderer,
    ui: Ui,
    text: TextSystem,
    input: InputState,
    layers: LayerStack,
    markup: String,
    phase: u8,
    dpi: f32,
    pointer: [f32; 2],
    shift: bool,
    dirty: bool,
    shown: bool,
    offscreen: bool,
}

impl DialogWindow {
    pub(super) fn new(light: bool) -> ara_ui::Result<Self> {
        let mut window = Self::create(false)?;
        window.ui.switch_theme(if light {
            ara_ui::theme::EditorTheme::Light
        } else {
            ara_ui::theme::EditorTheme::Dark
        })?;
        crate::native::frame::style(window.window.raw(), light);
        Ok(window)
    }

    fn create(offscreen: bool) -> ara_ui::Result<Self> {
        let window = Window::hidden("Update project layout | Ara", [648, 640])?;
        let _ = window.set_icon_png(include_bytes!(
            "../../../../../assets/brand/icons/engine/ara-engine-64.png"
        ));
        let dpi = window.display_scale().max(0.5);
        window.set_size([648., 640.].map(|value| (value * dpi).round() as u32))?;
        // SAFETY: This small modal owns its live SDL window on the creating thread.
        unsafe {
            sdl3_sys::video::SDL_SetWindowResizable(window.raw(), false);
        }
        let pointer = Window::global_mouse();
        let displays = Window::displays();
        if let Some(display) = displays
            .iter()
            .find(|display| {
                pointer[0] >= display[0] as f32
                    && pointer[1] >= display[1] as f32
                    && pointer[0] < (display[0] + display[2]) as f32
                    && pointer[1] < (display[1] + display[3]) as f32
            })
            .or(displays.first())
        {
            let size = window.size()?;
            window.set_position([
                display[0] + (display[2] - size[0] as i32) / 2,
                display[1] + (display[3] - size[1] as i32) / 2,
            ])?;
        }
        let device = crate::gpu::open_presentation(&window.extensions()?)?;
        // SAFETY: The adapter is dropped before this owned UI-only device; all calls stay on this thread.
        let mut adapter = unsafe { NativeUi::new(&device)? };
        adapter.set_dpi(dpi)?;
        // SAFETY: The live window/device outlive this present handle, retired in Drop.
        let present = if offscreen {
            None
        } else {
            Some(unsafe { adapter.create_present(&device, &window)? })
        };
        crate::native::frame::style(window.raw(), false);
        Ok(Self {
            adapter,
            present,
            device,
            window,
            renderer: Renderer::default(),
            ui: Ui::editor("<ui id='project-open' />", view::CSS, [648., 640.])?,
            text: crate::shell::text_system()?,
            input: InputState::default(),
            layers: LayerStack::default(),
            markup: String::new(),
            phase: u8::MAX,
            dpi,
            pointer: [-1.; 2],
            shift: false,
            dirty: true,
            shown: false,
            offscreen,
        })
    }

    pub(super) fn show(&mut self, project: &str, state: &State) -> ara_ui::Result<()> {
        let phase = match state {
            State::Checking => 0,
            State::Ready(_) => 1,
            State::Working { .. } => 2,
            State::Error(_) => 3,
        };
        let changed = self.phase != phase;
        if changed {
            self.layers.close(&mut self.ui, &mut self.input)?;
        }
        let markup = view::markup(project, state);
        if markup != self.markup {
            self.ui.reconcile(&markup)?;
            self.markup = markup;
            self.ui.frame(&mut self.text)?;
            self.dirty = true;
        }
        if changed {
            self.layers.open(
                &mut self.ui,
                &mut self.input,
                stable_id("project-migration"),
                LayerKind::Modal,
                [0., 0.],
                state.cancellable().then(|| stable_id("migration-cancel")),
            )?;
            self.phase = phase;
        }
        if self.input.focused().is_none() && state.cancellable() {
            self.input
                .focus(&mut self.ui, stable_id("migration-cancel"))?;
        }
        Ok(())
    }

    fn route(&mut self, event: InputEvent) -> ara_ui::Result<()> {
        self.layers.event(
            &mut self.ui,
            &mut self.input,
            event,
            &mut crate::native::OsClipboard,
        )
    }

    pub(super) fn actions(&mut self, state: &State) -> ara_ui::Result<Vec<Action>> {
        let mut actions = Vec::new();
        for event in self.window.poll()? {
            match event {
                Event::Quit => {
                    if state.cancellable() {
                        actions.push(Action::Cancel);
                    }
                }
                Event::Key {
                    scancode: 225 | 229,
                    pressed,
                    ..
                } => self.shift = pressed,
                Event::Key {
                    scancode: 41,
                    pressed: true,
                    ..
                } => {
                    if state.cancellable() {
                        actions.push(Action::Cancel);
                    }
                }
                Event::Key {
                    scancode: 40 | 88,
                    pressed: true,
                    ..
                } => self.route(InputEvent::Activate)?,
                Event::Key {
                    scancode: 43,
                    pressed: true,
                    ..
                } => self.route(InputEvent::Tab {
                    reverse: self.shift,
                })?,
                Event::MouseMotion { position, .. } => {
                    // SAFETY: This window is live on its owning SDL thread.
                    let density =
                        unsafe { sdl3_sys::video::SDL_GetWindowPixelDensity(self.window.raw()) }
                            .max(0.25);
                    self.pointer = position.map(|value| value * density / self.dpi);
                    self.route(InputEvent::PointerMove(self.pointer))?;
                }
                Event::MouseButton { button: 1, pressed } => self.route(if pressed {
                    InputEvent::PointerDown(self.pointer)
                } else {
                    InputEvent::PointerUp(self.pointer)
                })?,
                Event::MouseButton {
                    button: 4,
                    pressed: true,
                } => {
                    if state.cancellable() {
                        actions.push(Action::Cancel);
                    }
                }
                Event::Wheel(delta) => self.route(InputEvent::Wheel {
                    position: self.pointer,
                    delta: [-delta[0] * 48., -delta[1] * 48.],
                })?,
                Event::Resize(_) => {
                    let size = self.window.size()?.map(|value| value as f32 / self.dpi);
                    self.ui.resize(size)?;
                    // SAFETY: Retire only this dialog's present handle after its UI queue is idle.
                    unsafe {
                        self.adapter.wait_idle(&self.device)?;
                        if let Some(present) = &mut self.present {
                            self.adapter.retire_present(&self.device, present);
                        }
                        self.present =
                            Some(self.adapter.create_present(&self.device, &self.window)?);
                    }
                    self.dirty = true;
                }
                _ => {}
            }
        }
        for action in self.ui.drain_actions() {
            if action.node == stable_id("migration-cancel") {
                actions.push(Action::Cancel);
            } else if action.node == stable_id("migration-submit") {
                actions.push(Action::Migrate);
            } else if action.node == stable_id("migration-retry") {
                actions.push(Action::Retry);
            }
        }
        Ok(actions)
    }

    pub(super) fn frame(&mut self) -> ara_ui::Result<()> {
        let work = self.ui.frame(&mut self.text)?;
        if !self.dirty && work.paint_nodes == 0 && work.layout_passes == 0 {
            return Ok(());
        }
        self.renderer.render(
            &mut self.adapter,
            &self.ui.display,
            &self.text.assets,
            self.ui.size.map(|value| value.round() as u32),
            TextMode::Raster,
        )?;
        if let Some(present) = &mut self.present {
            // SAFETY: The UI submission completed and the live present/device belong to this dialog.
            unsafe {
                self.adapter.present(&self.device, present)?;
            }
        }
        if !self.shown && !self.offscreen {
            // SAFETY: A real interactive open explicitly requested this live SDL window. Headless paths never construct it.
            unsafe {
                sdl3_sys::video::SDL_ShowWindow(self.window.raw());
            }
            self.shown = true;
        }
        self.dirty = false;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn capture(
        project: &str,
        state: &State,
        directory: &std::path::Path,
        name: &str,
    ) -> ara_ui::Result<()> {
        let device = crate::gpu::open_presentation(&[])?;
        // SAFETY: The adapter is dropped before this owned UI-only device; no window or scene is created.
        let mut adapter = unsafe { NativeUi::new(&device)? };
        adapter.set_dpi(1.)?;
        let mut renderer = Renderer::default();
        let mut text = crate::shell::text_system()?;
        for (theme, light) in [("dark", false), ("light", true)] {
            let mut ui = Ui::editor(&view::markup(project, state), view::CSS, [648., 640.])?;
            ui.switch_theme(if light {
                ara_ui::theme::EditorTheme::Light
            } else {
                ara_ui::theme::EditorTheme::Dark
            })?;
            ui.frame(&mut text)?;
            renderer.render(
                &mut adapter,
                &ui.display,
                &text.assets,
                [648, 640],
                TextMode::Raster,
            )?;
            adapter
                .pixels()?
                .save(directory.join(format!("editor-migration-{name}-{theme}.png")))?;
        }
        // SAFETY: Complete the capture submission before this local adapter/device drop.
        unsafe {
            adapter.wait_idle(&device)?;
        }
        Ok(())
    }
}

impl Drop for DialogWindow {
    fn drop(&mut self) {
        // SAFETY: Complete this dialog's UI work and retire its own swapchain before the device/window drop.
        unsafe {
            let _ = self.adapter.wait_idle(&self.device);
            if let Some(present) = &mut self.present {
                self.adapter.retire_present(&self.device, present);
            }
        }
    }
}
