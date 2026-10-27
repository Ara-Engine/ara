//! A native game shell over the same World/Play roles used by the editor.
mod audio;
mod backdrop;
mod bot;
mod content;
mod events;
mod lobby;
mod menus;
mod session;
mod settings;
use crate::{
    Args, World, device,
    game_ui::{Router, Screen, Surface},
    input::{DeviceEvent, Mapper},
    net::Play,
};
use anyhow::{Context, Result};
use ara_input::{ActionValue, Injection};
use ara_platform::Event;
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::Presenter;
use settings::Settings;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(clap::Args)]
#[group(id = "GameOptions")]
pub(crate) struct Options {
    /// Run the project's native main menu and co-op game shell.
    #[arg(long)]
    pub game: bool,
    /// Source Ara project. Omit to play an exported pack beside the executable/--root.
    #[arg(long)]
    project: Option<PathBuf>,
    /// Start a renderer-free four-player authority.
    #[arg(long)]
    dedicated: bool,
    /// Direct-connect loopback lobby port.
    #[arg(long, default_value_t = 41090)]
    port: u16,
    /// Connect immediately to this loopback host (otherwise use the main menu).
    #[arg(long)]
    connect: Option<String>,
    /// Deterministic action bot for the offscreen co-op gate.
    #[arg(long, hide = true)]
    bot: bool,
    /// Drive the native solo menu route in the offscreen gate.
    #[arg(long, hide = true, requires = "bot", conflicts_with_all = ["connect", "dedicated"])]
    bot_solo: bool,
    /// Per-run scratch evidence/report directory for bots.
    #[arg(long, hide = true)]
    evidence: Option<PathBuf>,
}

/// Exported games with native menus open them on ordinary double-click launches.
pub(crate) fn has_menu(args: &Args) -> Result<bool> {
    if !args.standalone.opens_menu() {
        return Ok(false);
    }
    let root = match &args.standalone.root {
        Some(root) => root.to_owned(),
        None => std::env::current_exe()?
            .parent()
            .context("player folder")?
            .to_path_buf(),
    };
    if !root.join("ara-launch.json").is_file() {
        return Ok(false);
    }
    let Some(ui) = content::exported(&root)?.scene.ui else {
        return Ok(false);
    };
    Ok(ara_ui::source::compile(&ui.markup)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?
        .iter()
        .any(|element| element.id == "main-menu"))
}

pub(crate) fn run(args: Args) -> Result<()> {
    let mut settings = Settings::load()?;
    if let Some(ara_runtime_render::QualityChoice::Preset(preset)) = args.quality {
        use ara_runtime_render::QualityPreset;
        settings.quality = match preset {
            QualityPreset::Low => 0,
            QualityPreset::Medium => 1,
            QualityPreset::High => 2,
            QualityPreset::Epic => 3,
        };
    }
    if args.standalone.windowed {
        settings.window_mode = 0;
    }
    let root = args
        .game
        .project
        .clone()
        .or(args.standalone.root.clone())
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap_or_default()
                .parent()
                .map(PathBuf::from)
                .unwrap_or_default()
        });
    let initial = if args.game.project.is_some() {
        None
    } else {
        Some(content::exported(&root)?)
    };
    let ui = match &initial {
        Some(content) => content
            .scene
            .ui
            .clone()
            .context("this game has no native UI")?,
        None => content::ui(&root)?,
    };
    let no_window = args.game.dedicated || args.standalone.headless;
    let (window, presenter) = if no_window {
        (
            None,
            Presenter::new(settings.size(), Vec::new(), &args.gpu_cache),
        )
    } else {
        let options = ara_runtime_render::RenderOptions { ray_tracing: false };
        let (mut window, presenter) = if args.standalone.offscreen {
            device::Windowed::open_offscreen(
                "Outpost — Dusk shift",
                settings.size(),
                &args.gpu_cache,
                settings.quality(),
                options,
            )?
        } else {
            device::Windowed::open(
                "Outpost — Dusk shift",
                settings.size(),
                &args.gpu_cache,
                settings.quality(),
                options,
            )?
        };
        window.set_window_mode(settings.window_mode(), settings.size())?;
        window.game_ui = Some(Surface::plain(&ui, window.window_size()?)?);
        (Some(window), presenter)
    };
    let headless_ui = if window.is_none() {
        Some(Surface::plain(&ui, settings.size())?)
    } else {
        None
    };
    let ui_audio = if window.is_some() {
        audio::UiAudio::open(&ui.click)?
    } else {
        None
    };
    let mut app = App {
        root,
        project: args.game.project.is_some(),
        settings,
        window,
        presenter,
        headless_ui,
        router: Router::default(),
        mapper: Mapper::default(),
        screen: Screen::Main,
        settings_return: Screen::Main,
        world: None,
        loading: None,
        lobby: None,
        progress: None,
        port: args.game.port,
        frame: device::warm_frame(),
        audio: None,
        quit: false,
        bot: args
            .game
            .bot
            .then(|| bot::Bot::new(args.game.evidence.clone())),
        route: vec![],
        rebinding: None,
        ui_audio,
        backdrop: None,
        backdrop_loading: None,
        bot_start: None,
        evidence: args.game.evidence.clone(),
    };
    for (action, key) in &app.settings.keys {
        app.mapper.bind(action, *key);
    }
    app.screen(Screen::Main)?;
    app.init_settings()?;
    app.settings_labels()?;
    if args.game.dedicated {
        app.begin(ara_play::Role::Dedicated, None)?;
    }
    if let Some(address) = args.game.connect {
        if args.game.bot {
            if let Some(surface) = app.surface() {
                surface.label("address", &address)?;
            }
            app.click("join")?;
        } else {
            app.begin(ara_play::Role::Client, Some(address))?;
        }
    } else if args.game.bot && !args.game.dedicated {
        app.bot_start = Some(if args.game.bot_solo { "solo" } else { "host" }.into());
    }
    if !args.game.dedicated && app.world.is_none() && app.loading.is_none() {
        app.start_backdrop()?;
    }
    let mut previous = Instant::now();
    let mut frames = 0;
    while !app.quit && args.standalone.frames.is_none_or(|limit| frames < limit) {
        let now = Instant::now();
        app.tick(now.duration_since(previous))?;
        previous = now;
        frames += 1;
        if app.window.is_none() {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    if let Some(mut window) = app.window.take() {
        window.shutdown(&app.presenter)?;
    }
    Ok(())
}

struct App {
    root: PathBuf,
    project: bool,
    settings: Settings,
    window: Option<device::Windowed>,
    presenter: Presenter,
    headless_ui: Option<Surface>,
    router: Router,
    mapper: Mapper,
    screen: Screen,
    settings_return: Screen,
    world: Option<World>,
    loading: Option<std::sync::mpsc::Receiver<Result<session::Prepared>>>,
    progress: Option<std::sync::mpsc::Receiver<String>>,
    lobby: Option<lobby::Lobby>,
    port: u16,
    frame: RenderFrame,
    audio: Option<(crate::world::Audio, ara_audio::device::SdlBackend)>,
    quit: bool,
    bot: Option<bot::Bot>,
    route: Vec<String>,
    rebinding: Option<String>,
    ui_audio: Option<audio::UiAudio>,
    backdrop: Option<backdrop::Backdrop>,
    backdrop_loading: Option<std::sync::mpsc::Receiver<Result<backdrop::Prepared>>>,
    bot_start: Option<String>,
    evidence: Option<PathBuf>,
}

impl App {
    fn surface(&mut self) -> Option<&mut Surface> {
        match &mut self.window {
            Some(window) => window.game_ui.as_mut(),
            None => self.headless_ui.as_mut(),
        }
    }
    fn click(&mut self, name: &str) -> Result<()> {
        let actions = self.surface().context("native game surface")?.click(name)?;
        for action in actions {
            self.route.push(format!("action:{}", action.name));
            self.action(&action.name)?;
        }
        Ok(())
    }
    fn init_settings(&mut self) -> Result<()> {
        let values = [
            ("master", self.settings.master as f64),
            ("music", self.settings.music as f64),
            ("sfx", self.settings.sfx as f64),
            ("sensitivity", self.settings.sensitivity as f64),
        ];
        let invert = self.settings.invert;
        if let Some(surface) = self.surface() {
            surface.settings(&values, invert)?;
        }
        Ok(())
    }
    fn screen(&mut self, screen: Screen) -> Result<()> {
        self.screen = screen;
        self.route.push(format!("{screen:?}"));
        if let Some(surface) = self.surface() {
            surface.screen(screen)?;
        }
        self.mapper.apply(DeviceEvent::FocusLost);
        Ok(())
    }
    fn tick(&mut self, elapsed: Duration) -> Result<()> {
        if let Some(audio) = &mut self.ui_audio {
            audio.pump()?;
        }
        self.events()?;
        self.poll_backdrop()?;
        if let Some(progress) = &self.progress
            && let Some(latest) = progress.try_iter().last()
            && let Some(surface) = self.surface()
        {
            surface.label("loading-progress", &latest)?;
        }
        self.poll_loading()?;
        if self.screen == Screen::Loading
            && self
                .world
                .as_mut()
                .is_some_and(|world| world.runtime.synced())
        {
            self.screen(Screen::Lobby)?;
        }
        if self.bot.is_some()
            && self.screen == Screen::Lobby
            && self
                .world
                .as_ref()
                .is_some_and(|w| !matches!(&w.runtime,Play::Host(h) if h.dedicated()))
        {
            self.click("ready")?;
        }
        if let Some(world) = &mut self.world {
            if let (Some(lobby), Play::Host(host)) = (&mut self.lobby, &mut world.runtime) {
                lobby.poll(host)?;
            }
            let inputs = self.mapper.injections(world.runtime.player());
            if !inputs.is_empty() {
                world.runtime.input(0, inputs)?;
            }
            world.runtime.advance(elapsed)?;
            world.mark();
            if let Some(bot) = &mut self.bot {
                bot.drive(world)?;
            }
            let crew = if self.screen == Screen::Lobby {
                Some(lobby::players(world))
            } else {
                None
            };
            if self.window.is_some() {
                world.extract_frame(&mut self.frame);
            }
            if let Some(window) = &mut self.window {
                let size = window.window_size()?;
                if let Some(surface) = &mut window.game_ui {
                    surface.update(&world.runtime, world.runtime.player().0, size)?;
                    if let Some(crew) = &crew {
                        surface.label("lobby-players", crew)?;
                    }
                    surface.screen(self.screen)?;
                }
            }
            if let Some(surface) = &mut self.headless_ui {
                surface.update(&world.runtime, world.runtime.player().0, [1280, 720])?;
                if let Some(crew) = &crew {
                    surface.label("lobby-players", crew)?;
                }
                surface.screen(self.screen)?;
            }
        }
        let mut finished = None;
        if let Some(mut bot) = self.bot.take() {
            if let Some(world) = &mut self.world
                && bot.observe(world, &mut self.presenter, &mut self.frame, &self.route)?
            {
                finished = Some(if matches!(&world.runtime,Play::Host(h) if h.dedicated()) {
                    0
                } else {
                    world.runtime.player().0
                });
            }
            self.bot = Some(bot);
        }
        if let Some(player) = finished {
            if player != 0 {
                self.click("back")?;
            }
            if let Some(bot) = &self.bot {
                bot.returned_to_menu(player, &self.route)?;
            }
            self.quit = true;
            return Ok(());
        }
        if self.world.is_none()
            && let Some(backdrop) = &mut self.backdrop
        {
            backdrop.frame(&mut self.frame, elapsed)?;
        }
        if let Some(window) = &mut self.window {
            if let Some(world) = &self.world {
                window.present(&mut self.presenter, &self.frame, || {
                    world.fidelity_content(&self.frame)
                })?;
            } else if let Some(backdrop) = &self.backdrop {
                window.present(&mut self.presenter, &self.frame, || {
                    backdrop.world.fidelity_content(&self.frame)
                })?;
            } else {
                window.present(&mut self.presenter, &self.frame, || Ok(Default::default()))?;
            }
        }
        if let (Some(world), Some((audio, device))) = (&mut self.world, &mut self.audio) {
            let inputs = world.audio_inputs()?;
            crate::world::update_audio(
                audio,
                world.runtime.world(),
                &crate::world::listener(&self.frame),
                &inputs,
            )?;
            audio.volumes(self.settings.master, self.settings.music, self.settings.sfx)?;
            audio.pump(device, crate::AUDIO_WATERMARK)?;
        }
        Ok(())
    }
}
