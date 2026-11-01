//! Planar reflections' mirror view (`look::planar`): before a primary frame records,
//! the plane chosen from earlier frames' histograms is rendered from the camera
//! mirrored below it, through the shared secondary-view path of capture bakes
//! (`with_capture_view`: probe GI read, not relit), clipped to what lies above the
//! plane, and copied into the reflection atlas inside its own frame graph
//! (`Recipe::Planar`). The primary frame's reflection trace then reads it.
use super::*;
use crate::look::planar::Planar;
use crate::scene::Camera;

/// Distance (m) the camera must keep above the plane for a mirror view.
const ABOVE: f32 = 0.05;

/// What a primary frame's reflection trace reads of planar reflections.
pub(in crate::renderer) struct PlanarFrame {
    /// The histogram this frame counts floor candidates into.
    pub request: Option<usize>,
    /// The mirror view's view-projection and the plane's height, when one rendered.
    pub mirror: Option<(Mat4, f32)>,
}

impl Renderer {
    /// Planar reflections for a primary frame seen from `camera` (`settle`: waits for
    /// histograms in flight, so deterministic frames pick planes at a fixed latency):
    /// reads completed histograms, renders the chosen plane's mirror view into the
    /// atlas and arms this frame's histogram. `None` without planar reflections, on
    /// secondary views, debug views, the hybrid path (its rays own mirrors) and frames
    /// without the reflection trace (the first frame has no history).
    pub(in crate::renderer) fn prepare_planar(
        &mut self,
        camera: &Camera,
        settle: bool,
        mode: crate::capture::ViewMode,
    ) -> Result<Option<PlanarFrame>> {
        if self.secondary_active {
            return Ok(None);
        }
        let quality = self.look.quality;
        let settings = match quality.planar_reflections {
            Some(settings)
                if quality.reflections.is_some()
                    && quality.ray_queries.is_none()
                    && self.native.is_some() =>
            {
                settings
            }
            _ => {
                self.release_planar()?;
                return Ok(None);
            }
        };
        if !mode.lit() || self.targets.history.is_none() {
            return Ok(None);
        }
        if self.views.planar.is_none() {
            self.views.planar = Some(Planar::new(&mut self.gpu)?);
        }
        let planar = self.views.planar.as_mut().expect("created above");
        planar.observe(&mut self.gpu, settle)?;
        let request = planar.arm(&mut self.gpu, camera.position.y)?;
        let mirror = match planar.selector.plane() {
            Some(plane) if camera.position.y > plane + ABOVE => {
                Some((self.render_mirror(camera, plane, settings.scale)?, plane))
            }
            _ => None,
        };
        Ok(Some(PlanarFrame { request, mirror }))
    }

    /// Renders the mirror view of the plane at `plane` (m) for `camera` at `scale` of
    /// the render size into the atlas; returns its view-projection.
    fn render_mirror(&mut self, camera: &Camera, plane: f32, scale: f32) -> Result<Mat4> {
        ara_gpu::cpu_scope!("Raster.planar_mirror");
        let size = self
            .render
            .map(|v| ((v as f32 * scale).round() as u32).max(16));
        let mut state = match self.views.planar_view.take() {
            Some(state) if state.matches_extent(size, size) => state,
            Some(old) => {
                old.destroy(&mut self.gpu)?;
                ViewState::new(self, size, size)?
            }
            None => ViewState::new(self, size, size)?,
        };
        let mirrored = |p: Vec3| Vec3::new(p.x, 2. * plane - p.y, p.z);
        let spec = CameraSpec::LookAt {
            position: mirrored(camera.position),
            target: mirrored(camera.position + camera.forward),
            vertical_fov: 2. * (1. / camera.projection.y_axis.y).atan(),
        };
        let resolved = spec.resolve(size)?;
        // The surroundings as shaded, minus what needs history or several frames
        // (screen traces, virtual shadow pages, the near fog volume), the horizon, ray
        // queries and nested mirrors, as capture bakes do.
        let quality = crate::look::Quality {
            reflections: None,
            planar_reflections: None,
            screen_gi: None,
            volumetric_fog: None,
            ray_queries: None,
            horizon: None,
            virtual_shadows: None,
            depth_of_field: None,
            motion_blur: None,
            bloom: false,
            ..self.look.quality
        };
        if let Some(planar) = &mut self.views.planar {
            planar.atlas(&mut self.gpu, size)?;
        }
        let result = self.with_capture_view(&mut state, quality, |renderer| {
            renderer.mirror_plane = Some(plane);
            let rendered = renderer.render_frame(&spec, 1., true, crate::capture::ViewMode::Lit, false);
            renderer.mirror_plane = None;
            rendered
        });
        self.views.planar_view = Some(state);
        result?;
        Ok(resolved.projection * resolved.view)
    }

    /// Frees planar reflection state once the quality has none.
    fn release_planar(&mut self) -> Result<()> {
        if let Some(planar) = self.views.planar.take() {
            planar.destroy(&mut self.gpu)?;
        }
        if let Some(view) = self.views.planar_view.take() {
            view.destroy(&mut self.gpu)?;
        }
        Ok(())
    }
}
