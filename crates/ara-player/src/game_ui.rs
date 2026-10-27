//! Native game UI composition. The C# schema is the binding contract in every role.
use anyhow::{Context, Result, anyhow, ensure};
use ara_play::GameUi;
use ara_runtime::{Runtime, components::PlayerControl};
use ara_runtime_render::Presenter;
use ara_ui::{
    Ui,
    paint::{Box2, Draw},
    source::stable_id,
};
use ara_ui_render::{atlas::TextMode, gpu::Renderer};
use ara_ui_text::TextSystem;
use serde_json::Value;
mod anchors;
mod decoration;
mod screen;
#[cfg(test)]
mod tests;
pub(crate) use screen::{Router, Screen};

fn ui_error(error: Box<dyn std::error::Error>) -> anyhow::Error {
    anyhow!(error.to_string())
}

/// Retained native UI state, owned by the main-thread view, never by gameplay.
pub(crate) struct Surface {
    ui: Ui,
    text: TextSystem,
    source: GameUi,
    values: Vec<Option<Value>>,
    renderer: Renderer,
    gpu: Option<ara_editor_gpu::NativeUi>,
    screen: Screen,
}

impl Surface {
    pub(crate) fn resize(&mut self, size: [u32; 2]) -> Result<()> {
        if self.ui.size != size.map(|v| v as f32) {
            self.ui.resize(size.map(|v| v as f32)).map_err(ui_error)?;
        }
        Ok(())
    }
    pub(crate) fn new(source: &GameUi, runtime: &Runtime, size: [u32; 2]) -> Result<Self> {
        for binding in &source.bindings {
            let field = runtime
                .schema()
                .get(&binding.component)
                .and_then(|ty| ty.field(&binding.field))
                .with_context(|| {
                    format!(
                        "UI {}: missing {}.{} in C# schema",
                        binding.node, binding.component, binding.field
                    )
                })?;
            ensure!(
                !binding.visibility || field.ty == ara_schema::ValueType::Bool,
                "UI {}: visibility requires a C# bool",
                binding.node
            );
        }
        Self::plain(source, size)
    }

    pub(crate) fn plain(source: &GameUi, size: [u32; 2]) -> Result<Self> {
        let ui = Ui::parse(&source.markup, &source.stylesheet, size.map(|v| v as f32))
            .map_err(ui_error)?;
        for binding in &source.bindings {
            ensure!(
                ui.find(&binding.node).is_some(),
                "UI binding control {} is missing",
                binding.node
            );
        }
        let mut text = TextSystem::bundled().map_err(ui_error)?;
        // Renderer font assets appear lazily after shaping; keep the bundled
        // collection itself and register project faces alongside its fallbacks.
        for font in &source.fonts {
            ensure!(
                !text
                    .fonts
                    .collection
                    .register_fonts(font.clone().into(), None)
                    .is_empty(),
                "project UI font contains no valid faces"
            );
        }
        let mut renderer = Renderer::default();
        for (name, bytes) in &source.images {
            let image = image::load_from_memory(bytes)?.into_rgba8();
            renderer
                .image(
                    stable_id(name) as u32 & 0x7fff_ffff,
                    ara_ui_render::gpu::Image {
                        width: image.width(),
                        height: image.height(),
                        rgba: image.into_raw(),
                    },
                )
                .map_err(ui_error)?;
        }
        let mut surface = Self {
            ui,
            text,
            source: source.clone(),
            values: vec![None; source.bindings.len()],
            renderer,
            gpu: None,
            screen: Screen::Game,
        };
        surface.screen(Screen::Game)?;
        Ok(surface)
    }

    pub(crate) fn update(&mut self, runtime: &Runtime, player: u64, size: [u32; 2]) -> Result<()> {
        if self.ui.size != size.map(|v| v as f32) {
            self.ui.resize(size.map(|v| v as f32)).map_err(ui_error)?;
        }
        let session = runtime.play().session();
        let owner = runtime.world().iter_entities().find_map(|e| {
            e.get::<PlayerControl>()
                .filter(|c| c.player == player)
                .map(|_| e.id())
        });
        for (index, binding) in self.source.bindings.iter().enumerate() {
            let entity = if binding.entity == "player" {
                owner
            } else {
                session.entity(&binding.entity)
            };
            let value = entity
                .and_then(|e| ara_runtime_script::read(runtime.world(), e, &binding.component))
                .and_then(|v| v.get(&binding.field).cloned());
            if binding.world {
                anchors::update(&mut self.ui, runtime, binding, entity, owner, size)?;
            }
            if value == self.values[index] {
                continue;
            }
            let node = self.ui.find(&binding.node).context("validated UI node")?;
            if binding.visibility {
                self.ui
                    .set_visible(
                        node,
                        value.as_ref().and_then(Value::as_bool).unwrap_or(false),
                    )
                    .map_err(ui_error)?;
            } else {
                let raw = value
                    .as_ref()
                    .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
                    .unwrap_or_else(|| "—".into());
                let value = if binding.clock {
                    let seconds = value.as_ref().and_then(Value::as_u64).unwrap_or(0) / 60;
                    format!("{}:{:02}", seconds / 60, seconds % 60)
                } else {
                    binding.labels.get(&raw).cloned().unwrap_or(raw)
                };
                self.ui
                    .set_text(node, &binding.template.replace("{value}", &value))
                    .map_err(ui_error)?;
            }
            self.values[index] = value;
        }
        self.ui.frame(&mut self.text).map_err(ui_error)?;
        Ok(())
    }

    /// Composites directly from the completed scene image; no scene readback or CPU upload.
    /// Show the composite with [`Self::present`] or [`Self::publish`].
    pub(crate) fn render(&mut self, presenter: &Presenter) -> Result<()> {
        self.resize(presenter.size())?;
        self.ui.frame(&mut self.text).map_err(ui_error)?;
        decoration::apply(&mut self.ui, self.screen);
        let renderer = presenter.renderer().context("game UI needs a renderer")?;
        let image = presenter
            .output()
            .context("game UI needs a completed scene")?;
        if self.gpu.is_none() {
            // SAFETY: the window drops this surface before its presenter's device;
            // all UI and scene submissions occur sequentially on this thread.
            self.gpu = Some(unsafe {
                ara_editor_gpu::NativeUi::new(ara_editor_gpu::ui_device(renderer.gpu()))?
            });
        }
        let gpu = self.gpu.as_mut().context("native UI allocated")?;
        let id = stable_id("game-scene") as u32 & 0x7fff_ffff;
        // SAFETY: scene image/view and NativeUi share this presenter's live device.
        // The UI submission completes before the presenter can replace this image.
        unsafe {
            gpu.external_image(
                id,
                renderer.gpu().native_image(image)?,
                renderer.gpu().image_view(image)?,
            )?;
        }
        self.ui.display.commands.insert(
            0,
            Draw::Image {
                node: 0,
                rect: Box2 {
                    x: 0.,
                    y: 0.,
                    w: self.ui.size[0],
                    h: self.ui.size[1],
                },
                image: id,
                tint: ara_ui::style::Color(255, 255, 255, 255),
            },
        );
        let size = self.ui.size.map(|v| v as u32);
        let rendered = self
            .renderer
            .render(
                gpu,
                &self.ui.display,
                &self.text.assets,
                size,
                TextMode::Raster,
            )
            .map_err(ui_error);
        self.ui.display.commands.remove(0);
        rendered?;
        Ok(())
    }

    /// Presents the last composite into the window.
    /// # Safety
    /// `gpu` is the presenter's device, which created `present`; owners are live.
    pub(crate) unsafe fn present(
        &self,
        gpu: &ara_gpu::GpuDevice,
        present: &mut ara_platform::Present,
    ) -> Result<()> {
        let ui = self.gpu.as_ref().context("game UI has not rendered")?;
        // SAFETY: forwarded caller contract; the UI submission completed.
        unsafe { ui.present(ara_editor_gpu::ui_device(gpu), present) }
    }

    /// Publishes the last composite into an embedded player's shared ring.
    /// # Safety
    /// `ring` was created on `gpu`, the presenter's device; this thread owns its queues.
    pub(crate) unsafe fn publish(
        &self,
        gpu: &ara_gpu::GpuDevice,
        ring: &mut ara_gpu::external::ExportRing,
        tick: u64,
    ) -> Result<Option<ara_gpu::shared::Frame>> {
        let ui = self.gpu.as_ref().context("game UI has not rendered")?;
        // SAFETY: forwarded caller contract; the UI submission completed.
        unsafe { ui.publish(ara_editor_gpu::ui_device(gpu), ring, tick) }
    }
}

impl crate::device::Windowed {
    /// Pushes authoritative/replicated C# values to the local owner's retained UI.
    pub(crate) fn update_game_ui(
        &mut self,
        source: Option<&GameUi>,
        runtime: &Runtime,
        player: u64,
    ) -> Result<()> {
        let Some(source) = source else {
            self.game_ui = None;
            return Ok(());
        };
        let size = self.window_size()?;
        if self.game_ui.is_none() {
            self.game_ui = Some(Surface::new(source, runtime, size)?);
        }
        if let Some(surface) = &mut self.game_ui {
            surface.update(runtime, player, size)?;
        }
        Ok(())
    }
}

/// The same native markup/layout/text/render contract over the offscreen reference backend.
pub(crate) fn capture(
    source: &GameUi,
    runtime: &Runtime,
    player: u64,
    image: &mut image::RgbaImage,
) -> Result<()> {
    let size = [image.width(), image.height()];
    let overlay = snapshot(source, runtime, player, size)?;
    composite(image, &overlay);
    Ok(())
}

/// Freezes native layout and bindings before an asynchronous scene readback.
pub(crate) fn snapshot(
    source: &GameUi,
    runtime: &Runtime,
    player: u64,
    size: [u32; 2],
) -> Result<image::RgbaImage> {
    let mut surface = Surface::new(source, runtime, size)?;
    surface.update(runtime, player, size)?;
    let mut cpu = ara_ui_render::CpuGpu::default();
    surface
        .renderer
        .render(
            &mut cpu,
            &surface.ui.display,
            &surface.text.assets,
            size,
            TextMode::Raster,
        )
        .map_err(ui_error)?;
    let pixmap = cpu.pixmap().map_err(ui_error)?;
    image::RgbaImage::from_raw(size[0], size[1], pixmap.data().to_vec())
        .context("native UI snapshot extent")
}

/// Composites the native renderer's premultiplied pixels over a scene readback.
pub(crate) fn composite(image: &mut image::RgbaImage, overlay: &image::RgbaImage) {
    for (target, source) in image.pixels_mut().zip(overlay.pixels()) {
        let alpha = u32::from(source.0[3]);
        let rgb = [source.0[0], source.0[1], source.0[2]];
        for (channel, source) in target.0[..3].iter_mut().zip(rgb) {
            *channel = (u32::from(source) + (u32::from(*channel) * (255 - alpha) + 127) / 255)
                .min(255) as u8;
        }
        target.0[3] = 255;
    }
}
