//! SDL events feed native UI controls or replayable gameplay actions.
use super::*;
impl App {
    pub(super) fn events(&mut self) -> Result<()> {
        let completed = self.world.as_ref().is_some_and(|world| {
            let mission = bot::component(&world.runtime, "e_run", "SalvageRun");
            mission["won"] == true || mission["lost"] == true
        });
        if completed || self.screen != Screen::Game {
            self.mapper.apply(DeviceEvent::FocusLost);
        }
        if let Some(surface) = self
            .window
            .as_mut()
            .and_then(|window| window.game_ui.as_mut())
        {
            self.router.sync(surface)?;
        }
        self.sync_text_input()?;
        let Some(window) = &mut self.window else {
            return Ok(());
        };
        let events = window.raw_events()?;
        for event in events {
            if matches!(event, Event::FocusLost) {
                self.mapper.apply(DeviceEvent::FocusLost);
                self.rebinding = None;
            }
            if let Event::Key {
                scancode,
                pressed: true,
                repeat: false,
            } = event
                && let Some(action) = self.rebinding.take()
            {
                if scancode != 41 {
                    self.mapper.bind(&action, scancode);
                    self.settings.keys.insert(action, scancode);
                }
                if let Some(surface) = self.surface() {
                    surface.label("binding-help", "Choose a control to rebind.")?;
                }
                self.settings_labels()?;
                continue;
            }
            if matches!(event, Event::Quit) {
                self.quit = true;
            }
            if matches!(
                event,
                Event::Key {
                    scancode: 41,
                    pressed: true,
                    repeat: false
                }
            ) {
                if self.router.composing() {
                    if let Some(surface) = self
                        .window
                        .as_mut()
                        .and_then(|window| window.game_ui.as_mut())
                    {
                        self.router.event(surface, &event)?;
                    }
                    continue;
                }
                match self.screen {
                    Screen::Game => {
                        if let Some(world) = &mut self.world
                            && !world.runtime.networked()
                        {
                            world.runtime.pause()?;
                        }
                        self.screen(Screen::Pause)?;
                    }
                    Screen::Pause => self.action("resume")?,
                    Screen::Settings => self.screen(self.settings_return)?,
                    _ => {}
                }
                continue;
            }
            let mut ui_handled = false;
            if let Some(surface) = self
                .window
                .as_mut()
                .and_then(|window| window.game_ui.as_mut())
            {
                ui_handled = self.router.event(surface, &event)?;
                if self.router.text_active(surface) {
                    self.mapper.apply(DeviceEvent::FocusLost);
                }
                let actions = self.router.actions(surface);
                for action in actions {
                    self.action(&action.name)?;
                }
            }
            if self.screen == Screen::Game && !completed && !ui_handled {
                match event {
                    Event::Key {
                        scancode,
                        pressed,
                        repeat: false,
                    } => self.mapper.apply(DeviceEvent::Key {
                        scancode,
                        down: pressed,
                    }),
                    Event::MouseButton { button, pressed } => {
                        self.mapper.apply(DeviceEvent::MouseButton {
                            button,
                            down: pressed,
                        })
                    }
                    Event::MouseMotion { delta, .. } => {
                        self.mapper.apply(DeviceEvent::MouseMotion {
                            dx: delta[0] * self.settings.sensitivity,
                            dy: delta[1]
                                * self.settings.sensitivity
                                * if self.settings.invert { -1. } else { 1. },
                        })
                    }
                    _ => {}
                }
            }
        }
        let actions = self
            .window
            .as_mut()
            .and_then(|w| w.game_ui.as_mut())
            .map(|s| self.router.actions(s))
            .unwrap_or_default();
        for action in actions {
            self.action(&action.name)?;
        }
        self.sync_text_input()?;
        Ok(())
    }

    fn sync_text_input(&self) -> Result<()> {
        if let Some(window) = &self.window {
            let active = window
                .game_ui
                .as_ref()
                .is_some_and(|surface| self.router.text_active(surface));
            window.ui_text_input(active)?;
        }
        Ok(())
    }
}
