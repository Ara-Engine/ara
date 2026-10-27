//! Screen navigation and native event routing, independent of simulation state.
use super::*;
use ara_ui::input::{InputEvent, InputState};
use ara_ui::native_input::{Router as NativeInput, TextSource};
use ara_ui_text::Clipboard;

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
    native: NativeInput,
    screen: Option<Screen>,
    scope: Option<u64>,
}

struct OsClipboard;
impl Clipboard for OsClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        ara_platform::text::read_clipboard().map_err(|error| error.to_string().into())
    }
    fn write(&mut self, text: &str) -> ara_ui_text::Result<()> {
        ara_platform::text::write_clipboard(text).map_err(|error| error.to_string().into())
    }
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
    pub(crate) fn sync(&mut self, surface: &mut Surface) -> Result<()> {
        self.sync_with_clipboard(surface, &mut OsClipboard)
    }

    fn sync_with_clipboard(
        &mut self,
        surface: &mut Surface,
        clipboard: &mut dyn Clipboard,
    ) -> Result<()> {
        if self.screen == Some(surface.screen)
            && self
                .scope
                .is_some_and(|root| surface.ui.by_id(root).is_some())
        {
            return Ok(());
        }
        self.native
            .focus_lost(&mut surface.ui, &mut self.state, clipboard)
            .map_err(ui_error)?;
        if let Some(root) = self.scope.take() {
            self.state.remove_scope(&mut surface.ui, root);
        }
        let name = match surface.screen {
            Screen::Main => "main-menu",
            Screen::Lobby => "lobby",
            Screen::Loading => "loading",
            Screen::Settings => "settings",
            Screen::Pause => "pause",
            Screen::Game => "hud",
        };
        let index = surface.ui.find(name).unwrap_or(0);
        let root = stable_id(&surface.ui.nodes[index].element.id);
        self.state
            .push(&mut surface.ui, root, None, surface.screen != Screen::Game)
            .map_err(ui_error)?;
        if surface.screen == Screen::Game {
            self.state.blur(&mut surface.ui);
        }
        self.scope = Some(root);
        self.screen = Some(surface.screen);
        Ok(())
    }

    pub(crate) fn text_active(&self, surface: &Surface) -> bool {
        self.native.text_field(&surface.ui, &self.state).is_some()
    }

    pub(crate) fn composing(&self) -> bool {
        self.native.composing()
    }

    pub(crate) fn event(
        &mut self,
        surface: &mut Surface,
        event: &ara_platform::Event,
    ) -> Result<bool> {
        self.event_with_clipboard(surface, event, &mut OsClipboard)
    }

    pub(super) fn event_with_clipboard(
        &mut self,
        surface: &mut Surface,
        event: &ara_platform::Event,
        clipboard: &mut dyn Clipboard,
    ) -> Result<bool> {
        use ara_platform::Event;
        self.sync_with_clipboard(surface, clipboard)?;
        let mut handled = false;
        let input = match event {
            Event::MouseMotion { position, .. } => {
                self.pointer = *position;
                handled = self.state.captured().is_some()
                    || surface.ui.hit_test(position[0], position[1]).is_some();
                Some(InputEvent::PointerMove(*position))
            }
            Event::MouseButton {
                button: 1,
                pressed: true,
            } => {
                handled = surface
                    .ui
                    .hit_test(self.pointer[0], self.pointer[1])
                    .is_some();
                if !handled {
                    self.state.blur(&mut surface.ui);
                }
                Some(InputEvent::PointerDown(self.pointer))
            }
            Event::MouseButton {
                button: 1,
                pressed: false,
            } => {
                handled = self.state.captured().is_some();
                Some(InputEvent::PointerUp(self.pointer))
            }
            Event::Key {
                scancode,
                pressed,
                repeat,
            } => {
                handled = self
                    .native
                    .key(
                        &mut surface.ui,
                        &mut self.state,
                        *scancode,
                        *pressed,
                        *repeat,
                        clipboard,
                    )
                    .map_err(ui_error)?;
                None
            }
            Event::TextInput { text } => {
                handled = self
                    .native
                    .commit(
                        &mut surface.ui,
                        &mut self.state,
                        TextSource::Sdl,
                        text,
                        clipboard,
                    )
                    .map_err(ui_error)?;
                None
            }
            Event::TextEditing {
                text,
                start,
                length,
            } => {
                handled = self
                    .native
                    .preedit(
                        &mut surface.ui,
                        &mut self.state,
                        TextSource::Sdl,
                        text,
                        *start,
                        *length,
                        clipboard,
                    )
                    .map_err(ui_error)?;
                None
            }
            Event::FocusLost => {
                self.native
                    .focus_lost(&mut surface.ui, &mut self.state, clipboard)
                    .map_err(ui_error)?;
                handled = true;
                None
            }
            Event::Wheel(delta) => {
                handled = surface.ui.paint_order.iter().any(|i| {
                    surface.ui.scroll_position(*i).is_some()
                        && surface.ui.nodes[*i]
                            .rect
                            .intersection(surface.ui.nodes[*i].clip)
                            .contains(self.pointer[0], self.pointer[1])
                });
                Some(InputEvent::Wheel {
                    position: self.pointer,
                    delta: delta.map(|v| -v * 32.),
                })
            }
            _ => None,
        };
        if let Some(input) = input {
            self.state
                .event(&mut surface.ui, input, clipboard)
                .map_err(ui_error)?;
        }
        self.native
            .sync_focus(&mut surface.ui, &self.state, clipboard)
            .map_err(ui_error)?;
        Ok(handled || surface.screen != Screen::Game)
    }

    pub(crate) fn actions(&mut self, surface: &mut Surface) -> Vec<ara_ui::binding::Action> {
        surface.ui.drain_actions().collect()
    }
}

#[cfg(test)]
mod native_tests;
