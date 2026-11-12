//! The Scene view's GPU compositor: the renderer's image with the editor's scene overlays
//! (grid, selection and agent outlines by entity, the transform gizmo) drawn over it on the
//! GPU UI adapter, then presented into the studio's native layer. The overlays are the same
//! `crate::overlay` geometry and shaders.

use anyhow::Result;
use ara_ui::paint::{Box2, DisplayList, Draw};
use ara_ui::style::Color;
use ara_ui_render::atlas::TextMode;
use ara_ui_render::gpu::Renderer as UiRenderer;

use super::viewport::IdHost;
use crate::gpu::{IdHit, IdPick, NativeUi, ui_device};
use crate::overlay::{self, GizmoState, Outlines};
use crate::state::EditorState;
use crate::world::DocumentView;

/// Composites and presents the Scene layer. Create it once the renderer is ready; drop it
/// (after [`Self::retire`]) before the renderer.
pub(super) struct Compositor {
    ui: NativeUi,
    paint: UiRenderer,
    outlines: Outlines,
    /// Debug (`ARA_SCENE_DUMP=<file.png>`): each composed Scene frame, with its overlays,
    /// is read back and written there. The window captures never include the overlays.
    dump: Option<std::path::PathBuf>,
    sheet: ara_ui::style::Sheet,
    light: bool,
    present: Option<(ara_platform::Present, [u32; 2])>,
    #[cfg(any(windows, target_os = "linux"))]
    scene_stall: Option<(std::time::Instant, bool)>,
}

impl Compositor {
    /// Release Scene images before reuse, waiting at most 1 ms for the whole gate.
    /// A persistent stall skips Scene passes and logs once while events keep pumping.
    #[cfg(any(windows, target_os = "linux"))]
    pub fn scene_ready(&mut self, renderer: &super::device::SceneDevice) -> Result<bool> {
        let started = std::time::Instant::now();
        let budget = std::time::Duration::from_millis(1);
        let mut ready = self.ui.wait_previous_for(budget)?;
        // A bounded present can return with its blit still reading the UI target.
        if ready && let Some((present, _)) = &mut self.present {
            // SAFETY: this compositor retains the presentation and its creating device.
            ready = unsafe {
                present.settled(renderer.gpu(), budget.saturating_sub(started.elapsed()))?
            };
        }
        if ready {
            self.ui.order_before_scene(ui_device(renderer.gpu()))?;
            self.scene_stall = None;
        } else {
            let (since, reported) = self.scene_stall.get_or_insert((started, false));
            if !*reported && since.elapsed() >= std::time::Duration::from_millis(250) {
                eprintln!(
                    "Scene GPU read is still pending; skipping Scene passes while pumping events"
                );
                *reported = true;
            }
        }
        Ok(ready)
    }

    /// An adapter on the renderer's GPU, themed for `light`.
    /// # Safety
    /// The renderer outlives the compositor; one thread submits for both.
    pub unsafe fn new(renderer: &super::device::SceneDevice, light: bool) -> Result<Self> {
        // SAFETY: forwarded caller contract.
        let mut ui = unsafe { NativeUi::new(ui_device(renderer.gpu()))? };
        // Submissions return at once; the next frame checks the last one finished (the
        // UI thread never waits on the GPU).
        ui.set_deferred_completion(true)?;
        // Debug: `ARA_SCENE_HOVER=u,v` rests the pointer there, for captures of the hover.
        let hover = std::env::var("ARA_SCENE_HOVER").ok().and_then(|v| {
            let (u, v) = v.split_once(',')?;
            Some([u.trim().parse().ok()?, v.trim().parse().ok()?])
        });
        let mut outlines = Outlines::default();
        outlines.hover = hover;
        let mut paint = UiRenderer::default();
        paint
            .external_image(overlay::viewport_image())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self {
            ui,
            paint,
            outlines,
            dump: std::env::var_os("ARA_SCENE_DUMP").map(Into::into),
            sheet: sheet(light)?,
            light,
            present: None,
            #[cfg(any(windows, target_os = "linux"))]
            scene_stall: None,
        })
    }

    /// The map from presented renderer entities to scene addresses, brought up to date
    /// (Drop to Surface reads what the rays hit through it).
    pub fn channels(
        &mut self,
        entities: &[(u64, String)],
        view: &DocumentView,
        revision: u64,
    ) -> &crate::world::channels::Channels {
        self.outlines.channels.update(
            crate::world::EditWorldView::entities(view),
            revision,
            entities,
        );
        &self.outlines.channels
    }

    /// The entity the click at physical pixel `at` lands on, through the channel map (a model's
    /// parts and a terrain's tiles pick their entity). `host` answers from the Scene's own ids
    /// (Renderlab's frame); without one nothing is drawn yet and the click lands on nothing.
    pub fn pick(
        &mut self,
        host: Option<IdHost<'_>>,
        view: &DocumentView,
        revision: u64,
        at: [u32; 2],
    ) -> Result<Option<String>> {
        self.outlines.channels.update(
            crate::world::EditWorldView::entities(view),
            revision,
            host.as_ref().map_or(&[], |host| host.entities),
        );
        let id = match host {
            Some(mut host) => hit_entity(host.id_at(at)?, host.entities),
            None => None,
        };
        Ok(id
            .and_then(|id| self.outlines.channels.target_of(id))
            .map(str::to_owned))
    }

    /// Draws the Scene's image on screen and the overlays for `state` at `extent` physical
    /// pixels (`scale` per logical px), and presents it into `layer`. `state` carries the
    /// camera and `gizmo` the drag pose of the frame on screen. False when nothing reached the screen (the last
    /// frame is still on the GPU, or the present was skipped): call again on a later pass.
    /// # Safety
    /// `layer` is the Scene view's live native layer (`present::Target`).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn frame(
        &mut self,
        renderer: &mut super::device::SceneDevice,
        scene: &mut crate::studio::viewport::SceneView,
        view: &DocumentView,
        state: &EditorState,
        changed: &std::collections::BTreeMap<u64, &str>,
        gizmo: GizmoState,
        revision: u64,
        layer: super::present::Target,
        extent: [u32; 2],
        scale: f32,
    ) -> Result<bool> {
        if !self
            .ui
            .wait_previous_for(std::time::Duration::from_millis(1))?
        {
            return Ok(false);
        }
        if self.light != state.light {
            self.sheet = sheet(state.light)?;
            self.light = state.light;
        }
        // The image on screen: the last completed frame of an asynchronous Scene, or the
        // published presentation of a synchronous one.
        scene
            .attach(&mut self.ui, renderer)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let logical = [extent[0] as f32 / scale, extent[1] as f32 / scale];
        let rect = Box2 {
            x: 0.0,
            y: 0.0,
            w: logical[0],
            h: logical[1],
        };
        let overlays = overlay::build(
            view,
            state,
            rect,
            scene.entities(),
            changed,
            gizmo,
            &mut self.outlines,
            revision,
            &self.sheet,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.outlines
            .upload(&mut self.paint, &mut self.ui)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.ui.set_dpi(scale)?;
        // The overlays read the packed ids rasterised beside Renderlab's image.
        self.ui.overlays(
            rect,
            crate::gpu::IdSource::Packed(overlay::ENTITY_IMAGE),
            overlays,
        );
        let list = DisplayList {
            commands: vec![Draw::Image {
                node: 1,
                rect,
                image: overlay::viewport_image(),
                tint: Color(255, 255, 255, 255),
            }],
            glyphs: Vec::new(),
        };
        // Metal orders the UI frame after Renderlab's last submission on the adapter's queue.
        #[cfg(target_os = "macos")]
        self.ui.order_after(renderer.gpu());
        let size = [
            logical[0].round().max(1.0) as u32,
            logical[1].round().max(1.0) as u32,
        ];
        super::trace::time("scene paint", std::time::Duration::from_millis(3), || {
            self.paint
                .render(&mut self.ui, &list, &[], size, TextMode::Raster)
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(path) = &self.dump {
            self.ui
                .pixels()?
                .save(path)
                .map_err(|e| anyhow::anyhow!("ARA_SCENE_DUMP: {e}"))?;
        }
        let present = match self.present.take() {
            Some((mut present, was)) => {
                if was != extent {
                    super::present::resize(&mut present, extent)?;
                }
                present
            }
            // SAFETY: forwarded caller contract.
            None => unsafe { super::present::create(&self.ui, renderer, layer, extent)? },
        };
        let (present, _) = self.present.insert((present, extent));
        // SAFETY: the UI submission is ordered before the blit; the target is CopySource.
        super::trace::time("scene present", std::time::Duration::from_millis(3), || {
            // SAFETY: the UI submission is ordered before the blit; the target is CopySource.
            unsafe { self.ui.present(ui_device(renderer.gpu()), present) }
        })?;
        Ok(super::present::landed(present))
    }

    /// Releases the layer presentation; call before the window closes.
    pub fn retire(&mut self, renderer: &super::device::SceneDevice) {
        let _ = self.ui.wait_idle(ui_device(renderer.gpu()));
        if let Some((mut present, _)) = self.present.take() {
            // SAFETY: the adapter is idle and the presentation is not used again.
            unsafe {
                self.ui
                    .retire_present(ui_device(renderer.gpu()), &mut present)
            };
        }
    }
}

/// The renderer entity (`Renderer::entities()` identity) an id hit names. Nothing, and a
/// hit without an id (something is there that is not selectable), name none: the click
/// lands on empty space, as it does for an entity with no scene address.
fn hit_entity(hit: IdHit, entities: &[(u64, String)]) -> Option<u64> {
    let index = usize::try_from(hit.id()?.checked_sub(1)?).ok()?;
    entities.get(index).map(|(entity, _)| *entity)
}

/// The overlay colours come from the editor theme's tokens.
fn sheet(light: bool) -> Result<ara_ui::style::Sheet> {
    let theme = if light {
        ara_ui::theme::EditorTheme::Light
    } else {
        ara_ui::theme::EditorTheme::Dark
    };
    let tokens = theme.tokens().map_err(|e| anyhow::anyhow!("{e}"))?;
    ara_ui::style::Sheet::with_tokens("", tokens).map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use ara_frame::scene::{CameraSpec, Material, Scene, Texture};
    use ara_gpu::vulkan::QueueGate;
    use glam::Vec3;

    #[test]
    #[ignore = "requires Vulkan and the shared GPU lease"]
    fn scene_recycling_waits_for_the_compositor_read() -> Result<()> {
        let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/studio-compositor-handoff");
        let mut renderer = super::device::SceneDevice::new_raster(
            Scene {
                meshes: Vec::new(),
                materials: vec![Material {
                    textures: [0; 4],
                    ..Default::default()
                }],
                textures: vec![Texture {
                    size: [1, 1],
                    rgba: vec![255; 4],
                    srgb: false,
                }],
                lights: Vec::new(),
                sun_direction: Vec3::Y,
                sun_lux: Vec3::ZERO,
                environment: vec![[0.2, 0.3, 0.4, 1.]; 8],
                environment_size: [4, 2],
            },
            [64, 48],
            &[],
            cache,
        )?;
        renderer.render_view(
            &CameraSpec::LookAt {
                position: Vec3::new(0., 0., 3.),
                target: Vec3::ZERO,
                vertical_fov: 55f32.to_radians(),
            },
            1.,
            ara_frame::capture::ViewMode::Lit,
        )?;
        renderer.submit_scene_frame(|_| Ok(()))?;
        renderer.wait_scene_frame()?;
        // SAFETY: the renderer outlives its compositor on this owner thread.
        let mut compositor = unsafe { Compositor::new(&renderer, false)? };
        let image = renderer.output();
        // SAFETY: this retained, completed image stays live through the UI read.
        unsafe {
            compositor.ui.external_image(
                overlay::viewport_image(),
                renderer.gpu().native_image(image)?,
                renderer.gpu().image_view(image)?,
            )?;
        }
        let context = renderer.gpu().context();
        let mut gate = QueueGate::new(context, context.graphics)?;
        compositor
            .paint
            .render(
                &mut compositor.ui,
                &DisplayList {
                    commands: vec![Draw::Image {
                        node: 1,
                        rect: Box2 {
                            x: 0.,
                            y: 0.,
                            w: 64.,
                            h: 48.,
                        },
                        image: overlay::viewport_image(),
                        tint: Color(255, 255, 255, 255),
                    }],
                    glyphs: Vec::new(),
                },
                &[],
                [64, 48],
                TextMode::Raster,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let started = std::time::Instant::now();
        assert!(
            !compositor.scene_ready(&renderer)?,
            "a pending external read must prevent recycling and resize"
        );
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
        compositor.scene_stall = Some((started - std::time::Duration::from_secs(1), false));
        assert!(!compositor.scene_ready(&renderer)?);
        assert!(compositor.scene_stall.is_some_and(|(_, reported)| reported));
        gate.open()?;
        compositor.ui.wait_previous()?;
        assert!(compositor.scene_ready(&renderer)?);
        // Two publications recycle the pair that the compositor just released.
        for _ in 0..2 {
            renderer.submit_scene_frame(|_| Ok(()))?;
            renderer.wait_scene_frame()?;
        }
        renderer.resize([80, 60])?;
        compositor.retire(&renderer);
        renderer.gpu().assert_clean()?;
        Ok(())
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    #[test]
    fn a_hit_names_the_renderer_entity_of_its_id() {
        let entities = [(11, "a".to_owned()), (22, "b".to_owned())];
        assert_eq!(hit_entity(IdHit::Id(1), &entities), Some(11));
        assert_eq!(hit_entity(IdHit::Id(2), &entities), Some(22));
        // Past the table, background, and a hit without an id: empty space.
        assert_eq!(hit_entity(IdHit::Id(3), &entities), None);
        assert_eq!(hit_entity(IdHit::Nothing, &entities), None);
        assert_eq!(hit_entity(IdHit::Unselectable, &entities), None);
        assert_eq!(hit_entity(IdHit::from_packed([0xFF; 4]), &entities), None);
    }
}
