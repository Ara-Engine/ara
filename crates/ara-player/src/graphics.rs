//! Local user preferences, outside simulation snapshots and replication.
use crate::{device::Windowed, world::World};
use ara_graphics_settings::{Capabilities, Preset, Request, Service, Settings, Store, WindowMode};
use ara_runtime::plugin::{RuntimePlugin, Stage};
use ara_runtime_render::{Presenter, QualityChoice, QualityPreset, RenderOptions};
use ara_sim::{Session, StateProvider};
use serde_json::{Value, json};
use std::{any::TypeId, path::PathBuf};

#[derive(clap::Args, Default)]
#[group(id = "GraphicsOptions")]
pub(crate) struct Options {
    /// Complete graphics candidate JSON for this launch (does not save).
    #[arg(long)]
    graphics_settings: Option<PathBuf>,
    /// Render/display scale, 0.1..1.
    #[arg(long)]
    resolution_scale: Option<f32>,
    /// Presentation FPS cap; zero uncaps. Never changes the fixed tick.
    #[arg(long)]
    frame_rate_cap: Option<u32>,
    /// Vertical synchronization; omit to use user/project preferences.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    vsync: Option<bool>,
    /// Native display mode for this launch.
    #[arg(long, value_parser = ["windowed", "borderless", "fullscreen"])]
    pub(crate) window_mode: Option<String>,
    /// Platform monitor ID from play.graphics capabilities.
    #[arg(long)]
    monitor: Option<u32>,
}

pub(crate) struct LaunchOverrides {
    pub save_on_start: bool,
    pub options: Options,
    pub quality: Option<QualityChoice>,
    pub ray_tracing: Option<bool>,
    pub size: Option<String>,
    pub windowed: bool,
}

pub(crate) struct Plugin {
    service: Option<Service>,
}

impl Plugin {
    pub(crate) fn new(rendering: &ara_components::render::RenderSettings, game: &str) -> Self {
        let defaults = Settings {
            preset: from_choice(rendering.quality.into()),
            ray_tracing: rendering.ray_tracing,
            ..Default::default()
        };
        let store = match Store::for_game(game) {
            Ok(store) => Some(store),
            Err(diagnostic) => {
                eprintln!("{diagnostic}");
                None
            }
        };
        Self {
            service: Some(Service::new(defaults, store)),
        }
    }
}

impl RuntimePlugin for Plugin {
    fn name(&self) -> &'static str {
        "graphics_preferences"
    }
    fn install(&mut self, stage: Stage, session: &mut Session) -> ara_runtime::Result<()> {
        if stage == Stage::Gameplay {
            if let Some(service) = self.service.take() {
                session.world_mut().insert_resource(service);
            }
            session.add_provider(Preferences)?;
        }
        Ok(())
    }
}

struct Preferences;
impl StateProvider for Preferences {
    fn name(&self) -> &'static str {
        "graphics_preferences"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![TypeId::of::<Service>()]
    }
    fn capture(
        &self,
        _: &ara_sim::bevy_ecs::world::World,
    ) -> std::result::Result<Value, ara_sim::SimError> {
        Ok(Value::Null)
    }
    fn restore(
        &self,
        _: &mut ara_sim::bevy_ecs::world::World,
        _: &Value,
    ) -> std::result::Result<(), ara_sim::SimError> {
        Ok(())
    }
}

fn from_choice(choice: QualityChoice) -> Preset {
    match choice {
        QualityChoice::Auto => Preset::Auto,
        QualityChoice::Preset(QualityPreset::Low) => Preset::Low,
        QualityChoice::Preset(QualityPreset::Medium) => Preset::Medium,
        QualityChoice::Preset(QualityPreset::High) => Preset::High,
        QualityChoice::Preset(QualityPreset::Epic) => Preset::Epic,
    }
}

impl World {
    pub(crate) fn configure_graphics(
        &mut self,
        options: &Options,
        quality: Option<QualityChoice>,
        ray_tracing: Option<bool>,
        size: Option<&str>,
        windowed: bool,
    ) -> anyhow::Result<()> {
        let world = self.runtime.play_mut().session_mut().world_mut();
        let mut service = world.resource_mut::<Service>();
        let mut settings = match &options.graphics_settings {
            Some(path) => {
                let text = std::fs::read_to_string(path)?;
                anyhow::ensure!(
                    text.len() <= 65536,
                    "graphics settings override exceeds 64 KiB"
                );
                serde_json::from_value(ara_json::parse(&text)?)?
            }
            None => service.state().applied.clone(),
        };
        if let Some(choice) = quality {
            settings.preset = from_choice(choice);
        }
        if let Some(value) = ray_tracing {
            settings.ray_tracing = value;
        }
        if let Some(value) = options.resolution_scale {
            settings.resolution_scale = Some(value);
        }
        if let Some(value) = options.frame_rate_cap {
            settings.frame_rate_cap = value;
        }
        if let Some(value) = options.vsync {
            settings.vsync = value;
        }
        if let Some(value) = options.monitor {
            settings.monitor = value;
        }
        if let Some(value) = &options.window_mode {
            settings.window_mode = serde_json::from_value(json!(value))?;
        }
        if windowed {
            settings.window_mode = WindowMode::Windowed;
        }
        if let Some(size) = size {
            let size = crate::parse_size(size)?;
            [settings.width, settings.height] = size;
        }
        service.startup(settings);
        for diagnostic in &service.state().diagnostics {
            eprintln!("{diagnostic}");
        }
        Ok(())
    }

    /// Apply revisions between script callbacks and presentation, never while
    /// simulation or the read-only audio worker owns the world.
    pub(crate) fn sync_graphics(
        &mut self,
        mut presenter: Option<&mut Presenter>,
        window: Option<&mut Windowed>,
    ) -> anyhow::Result<()> {
        let simulation = self.runtime.play_mut().session_mut().world_mut();
        let mut service = simulation.resource_mut::<Service>();
        let refresh = service.take_capabilities_refresh();
        if refresh
            || !service.state().capabilities.device_known
            || self.graphics_revision != service.state().revision
        {
            let mut caps = presenter
                .as_ref()
                .map_or_else(Capabilities::default, |p| p.graphics_capabilities());
            if let Some(window) = &window {
                caps.display_available = window.graphics_display_available();
                caps.current_monitor = window.current_monitor();
                caps.modes = window.graphics_modes()?;
            }
            // HDR stays unavailable until the player has a complete HDR output
            // encoder (an HDR swapchain alone cannot encode its SDR scene/UI).
            service.set_capabilities(caps);
        }
        let state = service.state();
        if let Some(presenter) = presenter.as_deref_mut()
            && (self.graphics_revision != state.revision || presenter.graphics_settings().is_none())
        {
            presenter.set_graphics_settings(&state.applied);
            presenter.set_render_options(RenderOptions {
                ray_tracing: state.active_ray_tracing,
            });
        }
        if self.graphics_revision == state.revision {
            return Ok(());
        }
        if let Some(window) = window
            && let Err(error) = window.apply_graphics(&state.applied)
        {
            let diagnostic = service.reject_application(format!(
                "display change failed: {error:#}; restoring the previous graphics preferences"
            ));
            eprintln!("{diagnostic}");
            let restored = &service.state().applied;
            if let Err(error) = window.apply_graphics(restored) {
                eprintln!("{diagnostic}: rollback display: {error:#}");
            }
            if let Some(presenter) = presenter {
                presenter.set_graphics_settings(restored);
                presenter.set_render_options(RenderOptions {
                    ray_tracing: service.state().active_ray_tracing,
                });
            }
            // A failed rollback stays diagnosed until a new Apply. Retrying the
            // same unsupported display mode every frame would stall the menu.
            self.graphics_revision = service.state().revision;
            return Ok(());
        }
        self.graphics_revision = state.revision;
        Ok(())
    }

    pub(crate) fn graphics(
        &mut self,
        request: Request,
        presenter: Option<&Presenter>,
    ) -> anyhow::Result<Value> {
        let state = self
            .runtime
            .play_mut()
            .session_mut()
            .world_mut()
            .resource_mut::<Service>()
            .request(request)
            .map_err(|d| anyhow::anyhow!(d.to_string()))?;
        let quality = presenter
            .and_then(Presenter::quality)
            .map(|preset| preset.name());
        Ok(json!({"state":state,"quality":quality}))
    }
}
