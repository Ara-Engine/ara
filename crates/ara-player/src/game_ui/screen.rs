//! Screen navigation and native event routing, independent of simulation state.
use super::*;
use ara_ui::input::{InputEvent, InputState};
use ara_ui_text::{Clipboard, EditEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    Main,
    Lobby,
    Loading,
    Settings,
    Pause,
    Game,
}

#[derive(Default)]
pub(crate) struct Router {
    state: InputState,
    pointer: [f32; 2],
    clipboard: MemoryClipboard,
}

#[derive(Default)]
struct MemoryClipboard(String);
impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
    }
}

impl Surface {
    pub(crate) fn settings(&mut self, values: &[(&str, f64)], invert: bool) -> Result<()> {
        for (name, value) in values {
            if let Some(node) = self.ui.find(name) {
                self.ui.set_value(node, *value).map_err(ui_error)?;
            }
        }
        if let Some(node) = self.ui.find("invert") {
            self.ui.set_checked(node, invert).map_err(ui_error)?;
        }
        Ok(())
    }
    /// Headless route tests activate the actual laid-out native button.
    pub(crate) fn click(&mut self, name: &str) -> Result<Vec<ara_ui::binding::Action>> {
        self.ui.frame(&mut self.text).map_err(ui_error)?;
        let node = self
            .ui
            .find(name)
            .with_context(|| format!("missing route control {name}"))?;
        let rect = self.ui.nodes[node].rect;
        ensure!(
            rect.w > 0. && rect.h > 0.,
            "route control {name} is not laid out"
        );
        let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        let mut state = InputState::default();
        let mut clipboard = MemoryClipboard::default();
        state
            .event(&mut self.ui, InputEvent::PointerDown(point), &mut clipboard)
            .map_err(ui_error)?;
        state
            .event(&mut self.ui, InputEvent::PointerUp(point), &mut clipboard)
            .map_err(ui_error)?;
        let actions: Vec<_> = self.ui.drain_actions().collect();
        ensure!(
            !actions.is_empty(),
            "native control {name} did not activate"
        );
        Ok(actions)
    }
    #[cfg(test)]
    pub(crate) fn save(&mut self, path: &std::path::Path) -> Result<()> {
        let image = self.image()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        image.save(path)?;
        Ok(())
    }
    pub(crate) fn image(&mut self) -> Result<image::RgbaImage> {
        self.ui.frame(&mut self.text).map_err(ui_error)?;
        decoration::apply(&mut self.ui, self.screen);
        let size = self.ui.size.map(|v| v as u32);
        let mut cpu = ara_ui_render::CpuGpu::default();
        self.renderer.device_lost();
        self.renderer
            .render(
                &mut cpu,
                &self.ui.display,
                &self.text.assets,
                size,
                TextMode::Raster,
            )
            .map_err(ui_error)?;
        self.report_missing_images();
        self.renderer.device_lost();
        image::RgbaImage::from_raw(
            size[0],
            size[1],
            cpu.pixmap().map_err(ui_error)?.data().to_vec(),
        )
        .context("native UI image extent")
    }
    pub(crate) fn screen(&mut self, screen: Screen) -> Result<()> {
        self.screen = screen;
        for (name, show) in [
            ("main-menu", screen == Screen::Main),
            ("lobby", screen == Screen::Lobby),
            ("loading", screen == Screen::Loading),
            ("settings", screen == Screen::Settings),
            ("pause", screen == Screen::Pause),
            ("hud", matches!(screen, Screen::Game | Screen::Pause)),
        ] {
            if let Some(node) = self.ui.find(name) {
                self.ui.set_visible(node, show).map_err(ui_error)?;
            }
        }
        self.ui.frame(&mut self.text).map_err(ui_error)?;
        Ok(())
    }

    pub(crate) fn label(&mut self, name: &str, value: &str) -> Result<()> {
        if let Some(node) = self.ui.find(name) {
            self.ui.set_text(node, value).map_err(ui_error)?;
        }
        Ok(())
    }

    pub(crate) fn value(&self, name: &str) -> Option<Value> {
        use ara_ui::widgets::Widget;
        match self.ui.widgets.get(&stable_id(name))? {
            Widget::Input(text) => Some(Value::String(text.value().into())),
            Widget::Slider { value, .. } => Some(serde_json::json!(value)),
            Widget::Toggle(value) => Some(Value::Bool(*value)),
            _ => None,
        }
    }
}

impl Router {
    pub(crate) fn event(
        &mut self,
        surface: &mut Surface,
        event: &ara_platform::Event,
    ) -> Result<()> {
        use ara_platform::Event;
        let input = match *event {
            Event::MouseMotion { position, .. } => {
                self.pointer = position;
                Some(InputEvent::PointerMove(position))
            }
            Event::MouseButton {
                button: 1,
                pressed: true,
            } => Some(InputEvent::PointerDown(self.pointer)),
            Event::MouseButton {
                button: 1,
                pressed: false,
            } => Some(InputEvent::PointerUp(self.pointer)),
            Event::Key {
                scancode: 43,
                pressed: true,
                repeat: false,
            } => Some(InputEvent::Tab { reverse: false }),
            Event::Key {
                scancode: 40,
                pressed: true,
                repeat: false,
            } => Some(InputEvent::Activate),
            Event::Key {
                scancode: 42,
                pressed: true,
                ..
            } => Some(InputEvent::Edit(EditEvent::Backspace)),
            Event::Key {
                scancode,
                pressed: true,
                repeat: false,
            } => {
                // The direct-connect address is numeric. Native SDL text/IME is not
                // exposed by ara-platform yet; number/dot/colon editing stays usable.
                let text = match scancode {
                    30..=38 => Some(char::from(b'1' + (scancode - 30) as u8)),
                    39 => Some('0'),
                    55 | 99 => Some('.'),
                    51 => Some(':'),
                    _ => None,
                };
                text.map(|c| InputEvent::Edit(EditEvent::Insert(c.to_string())))
            }
            Event::Wheel(delta) => Some(InputEvent::Wheel {
                position: self.pointer,
                delta,
            }),
            _ => None,
        };
        if let Some(input) = input {
            self.state
                .event(&mut surface.ui, input, &mut self.clipboard)
                .map_err(ui_error)?;
        }
        Ok(())
    }

    pub(crate) fn actions(&mut self, surface: &mut Surface) -> Vec<ara_ui::binding::Action> {
        surface.ui.drain_actions().collect()
    }
}
