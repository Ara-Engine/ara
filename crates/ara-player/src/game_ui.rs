//! Native game UI composition. The C# schema is the binding contract in every role.
use anyhow::{Context, Result, anyhow, ensure};
use ara_play::GameUi;
use ara_runtime::{Runtime, components::PlayerControl};
use ara_runtime_render::Presenter;
use ara_ui::{Ui, paint::{Box2, Draw}, source::stable_id};
use ara_ui_render::{atlas::TextMode, gpu::Renderer};
use ara_ui_text::TextSystem;
use serde_json::Value;
mod screen;
pub(crate) use screen::{Screen, Router};

fn ui_error(error: Box<dyn std::error::Error>) -> anyhow::Error { anyhow!(error.to_string()) }

/// Retained native UI state, owned by the main-thread view, never by gameplay.
pub(crate) struct Surface {
    ui: Ui,
    text: TextSystem,
    source: GameUi,
    values: Vec<Option<Value>>,
    renderer: Renderer,
    gpu: Option<ara_editor_gpu::NativeUi>,
}

impl Surface {
    pub(crate) fn new(source: &GameUi, runtime: &Runtime, size: [u32; 2]) -> Result<Self> {
        for binding in &source.bindings {
            let field = runtime.schema().get(&binding.component)
                .and_then(|ty| ty.field(&binding.field))
                .with_context(|| format!("UI {}: missing {}.{} in C# schema", binding.node, binding.component, binding.field))?;
            ensure!(!binding.visibility || field.ty == ara_schema::ValueType::Bool,
                "UI {}: visibility requires a C# bool", binding.node);
        }
        Self::plain(source, size)
    }

    pub(crate) fn plain(source: &GameUi, size: [u32; 2]) -> Result<Self> {
        let ui = Ui::parse(&source.markup, &source.stylesheet, size.map(|v| v as f32)).map_err(ui_error)?;
        for binding in &source.bindings {
            ensure!(ui.find(&binding.node).is_some(), "UI binding control {} is missing", binding.node);
        }
        let mut surface = Self { ui, text: TextSystem::bundled().map_err(ui_error)?, source: source.clone(),
            values: vec![None; source.bindings.len()], renderer: Renderer::default(), gpu: None };
        surface.screen(Screen::Game)?;
        Ok(surface)
    }

    pub(crate) fn update(&mut self, runtime: &Runtime, player: u64, size: [u32; 2]) -> Result<()> {
        if self.ui.size != size.map(|v| v as f32) {
            self.ui.resize(size.map(|v| v as f32)).map_err(ui_error)?;
        }
        let session = runtime.play().session();
        let owner = runtime.world().iter_entities().find_map(|e| {
            e.get::<PlayerControl>().filter(|c| c.player == player).map(|_| e.id())
        });
        for (index, binding) in self.source.bindings.iter().enumerate() {
            let entity = if binding.entity == "player" { owner } else { session.entity(&binding.entity) };
            let value = entity.and_then(|e| ara_runtime_script::read(runtime.world(), e, &binding.component))
                .and_then(|v| v.get(&binding.field).cloned());
            if value == self.values[index] { continue; }
            let node = self.ui.find(&binding.node).context("validated UI node")?;
            if binding.visibility {
                self.ui.set_visible(node, value.as_ref().and_then(Value::as_bool).unwrap_or(false)).map_err(ui_error)?;
            } else {
                let value = value.as_ref().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned)).unwrap_or_else(|| "—".into());
                self.ui.set_text(node, &binding.template.replace("{value}", &value)).map_err(ui_error)?;
            }
            self.values[index] = value;
        }
        self.ui.frame(&mut self.text).map_err(ui_error)?;
        Ok(())
    }

    /// Composites directly from the completed scene image; no scene readback or CPU upload.
    pub(crate) fn render(&mut self, presenter: &Presenter) -> Result<(ara_gpu::vk::Image, [u32; 2])> {
        let renderer = presenter.renderer().context("game UI needs a renderer")?;
        let image = presenter.output().context("game UI needs a completed scene")?;
        if self.gpu.is_none() {
            // SAFETY: the window drops this surface before its presenter's device;
            // all UI and scene submissions occur sequentially on this thread.
            self.gpu = Some(unsafe { ara_editor_gpu::NativeUi::new(renderer.gpu().context())? });
        }
        let gpu = self.gpu.as_mut().context("native UI allocated")?;
        let id = stable_id("game-scene") as u32 & 0x7fff_ffff;
        // SAFETY: scene image/view and NativeUi share this presenter's live device.
        // The UI submission completes before the presenter can replace this image.
        unsafe { gpu.external_image(id, renderer.gpu().native_image(image)?, renderer.gpu().image_view(image)?)?; }
        self.ui.display.commands.insert(0, Draw::Image {
            node: 0, rect: Box2 { x: 0., y: 0., w: self.ui.size[0], h: self.ui.size[1] },
            image: id, tint: ara_ui::style::Color(255,255,255,255),
        });
        let size = self.ui.size.map(|v| v as u32);
        let rendered = self.renderer.render(gpu, &self.ui.display, &self.text.assets, size, TextMode::Raster).map_err(ui_error);
        self.ui.display.commands.remove(0);
        rendered?;
        gpu.output()
    }
}

impl crate::device::Windowed {
    /// Pushes authoritative/replicated C# values to the local owner's retained UI.
    pub(crate) fn update_game_ui(&mut self, source: Option<&GameUi>, runtime: &Runtime, player: u64) -> Result<()> {
        let Some(source) = source else { self.game_ui = None; return Ok(()); };
        let size = self.window_size()?;
        if self.game_ui.is_none() { self.game_ui = Some(Surface::new(source, runtime, size)?); }
        if let Some(surface) = &mut self.game_ui { surface.update(runtime, player, size)?; }
        Ok(())
    }
}

/// The same native markup/layout/text/render contract over the offscreen reference backend.
pub(crate) fn capture(source: &GameUi, runtime: &Runtime, player: u64, image: &mut image::RgbaImage) -> Result<()> {
    let size = [image.width(), image.height()];
    let mut surface = Surface::new(source, runtime, size)?;
    surface.update(runtime, player, size)?;
    let mut cpu = ara_ui_render::CpuGpu::default();
    surface.renderer.render(&mut cpu, &surface.ui.display, &surface.text.assets, size, TextMode::Raster).map_err(ui_error)?;
    let pixmap = cpu.pixmap().map_err(ui_error)?;
    for (target, source) in image.pixels_mut().zip(pixmap.pixels()) {
        let alpha = u32::from(source.alpha());
        let rgb = [source.red(), source.green(), source.blue()];
        for (channel, source) in target.0[..3].iter_mut().zip(rgb) {
            *channel = (u32::from(source) + (u32::from(*channel) * (255 - alpha) + 127) / 255).min(255) as u8;
        }
        target.0[3] = 255;
    }
    Ok(())
}
