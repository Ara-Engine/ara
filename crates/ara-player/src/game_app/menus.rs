//! Menu actions and persisted presentation preferences.
use super::*;
impl App {
    pub(super) fn action(&mut self, name: &str) -> Result<()> {
        if let Some(audio) = &mut self.ui_audio {
            audio.click(self.settings.master * self.settings.sfx * 0.4)?;
        }
        match name {
            "solo" => self.begin(ara_play::Role::Local, None)?,
            "host" => self.begin(ara_play::Role::Host, None)?,
            "join" => {
                let address = self
                    .surface()
                    .and_then(|s| s.value("address"))
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| format!("127.0.0.1:{}", self.port));
                self.begin(ara_play::Role::Client, Some(address))?;
            }
            "ready" => {
                if let Some(world) = &mut self.world {
                    world.runtime.input(
                        0,
                        vec![Injection {
                            player: world.runtime.player(),
                            action: "Interact".into(),
                            value: ActionValue::Button(true),
                            hold_ticks: 1,
                        }],
                    )?;
                    if !world.runtime.networked() {
                        world.runtime.resume()?;
                    }
                }
                self.screen(Screen::Game)?;
            }
            "settings" => {
                self.settings_return = self.screen;
                self.screen(Screen::Settings)?;
            }
            "quality" => {
                self.settings.quality = (self.settings.quality + 1) % 4;
                self.presenter.set_quality(self.settings.quality());
            }
            "resolution" => {
                self.settings.resolution = (self.settings.resolution + 1) % 3;
                if let Some(window) = &self.window {
                    window.set_window_mode(self.settings.window_mode(), self.settings.size())?;
                }
            }
            "window-mode" => {
                self.settings.window_mode = (self.settings.window_mode + 1) % 3;
                if let Some(window) = &self.window {
                    window.set_window_mode(self.settings.window_mode(), self.settings.size())?;
                }
            }
            "apply" => {
                self.read_settings();
                self.settings.save()?;
                self.screen(self.settings_return)?;
            }
            "resume" => {
                if let Some(world) = &mut self.world
                    && !world.runtime.networked()
                {
                    world.runtime.resume()?;
                }
                self.screen(Screen::Game)?;
            }
            "leave" => {
                self.world = None;
                self.audio = None;
                self.lobby = None;
                self.loading = None;
                self.screen(Screen::Main)?;
                if self.bot.is_none() {
                    self.start_backdrop()?;
                }
            }
            "quit" => self.quit = true,
            name if name.starts_with("rebind-") => {
                self.rebinding = Some(name.trim_start_matches("rebind-").to_owned());
                if let Some(surface) = self.surface() {
                    surface.label("binding-help", "Press a key. Escape cancels.")?;
                }
            }
            _ => self.read_settings(),
        }
        self.settings_labels()
    }
    pub(super) fn read_settings(&mut self) {
        let Some(surface) = self.window.as_mut().and_then(|w| w.game_ui.as_mut()) else {
            return;
        };
        for (name, target) in [
            ("master", &mut self.settings.master),
            ("music", &mut self.settings.music),
            ("sfx", &mut self.settings.sfx),
            ("sensitivity", &mut self.settings.sensitivity),
        ] {
            if let Some(value) = surface.value(name).and_then(|v| v.as_f64()) {
                *target = value as f32;
            }
        }
        if let Some(value) = surface.value("invert").and_then(|v| v.as_bool()) {
            self.settings.invert = value;
        }
    }
    pub(super) fn settings_labels(&mut self) -> Result<()> {
        let quality = format!(
            "Graphics: {}",
            Settings::QUALITIES[self.settings.quality % 4]
        );
        let [w, h] = self.settings.size();
        let resolution = format!("Resolution: {w} x {h}");
        let mode = format!("Window: {:?}", self.settings.window_mode());
        let bindings: Vec<_> = [
            ("Forward", 26),
            ("Backward", 22),
            ("Left", 4),
            ("Right", 7),
            ("Jump", 44),
            ("Sprint", 225),
            ("Interact", 8),
            ("Inventory", 43),
        ]
        .into_iter()
        .map(|(action, default)| {
            (
                format!("rebind-{action}"),
                format!(
                    "{action}: {}",
                    key_name(self.settings.keys.get(action).copied().unwrap_or(default))
                ),
            )
        })
        .collect();
        if let Some(surface) = self.surface() {
            surface.label("quality", &quality)?;
            surface.label("resolution", &resolution)?;
            surface.label("window-mode", &mode)?;
        }
        if let Some(surface) = self.surface() {
            for (name, label) in bindings {
                surface.label(&name, &label)?;
            }
        }
        Ok(())
    }
}

fn key_name(key: i32) -> String {
    match key {
        4..=29 => char::from(b'A' + (key - 4) as u8).to_string(),
        30..=38 => char::from(b'1' + (key - 30) as u8).to_string(),
        39 => "0".into(),
        40 => "Enter".into(),
        42 => "Backspace".into(),
        43 => "Tab".into(),
        44 => "Space".into(),
        79 => "Right".into(),
        80 => "Left".into(),
        81 => "Down".into(),
        82 => "Up".into(),
        224 | 228 => "Ctrl".into(),
        225 | 229 => "Shift".into(),
        226 | 230 => "Alt".into(),
        _ => format!("Key {key}"),
    }
}
