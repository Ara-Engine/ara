use crate::layout::*;
use anyhow::{Result, ensure};
use ara_gpu::{Access, Buffer, Device, Image, Pipeline, Queue, Resource, vulkan::VulkanDevice};
use ara_render::{
    capture::{Capture, CaptureRequest, FeatureCapture, ShaderMetadata},
    graph::State,
    scene::CameraSpec,
};
use ara_vfx::{Effect, Module};
use glam::Vec3;

/// Per-view extracted data; one instance may render into several views sequentially.
pub struct View {
    /// Read-only scene background, copied into the feature's own HDR attachment.
    pub scene_background: Option<SceneColor>,
    /// Fidelity depth is positive view distance; the baseline uses reversed Z.
    pub linear_depth: bool,
    /// Scene sun, ambient fallback and optional directional-probe field.
    pub lighting: Lighting,
    /// Camera using Ara's reversed-Z projection.
    pub camera: CameraSpec,
    /// Positive linear exposure before tone mapping.
    pub exposure: f32,
    /// Use a dedicated compute family. Enable only after measuring this workload.
    pub async_compute: bool,
    /// Optional same-sized scene depth. Graphics ownership is returned as ShaderRead.
    pub scene_depth: Option<SceneDepth>,
    /// Optional HDR scene color to composite into; requires scene_depth.
    pub scene_color: Option<SceneColor>,
    /// Where the effect sits in the world. `camera`, scene depth and probes are
    /// world-space; the system simulates and draws in its own local space.
    pub placement: Placement,
    /// Multiplier on emitted and lit particle radiance (not on opacity).
    pub intensity: f32,
}

/// An effect instance's world origin and uniform scale (`VisualEffect.scale`):
/// local positions, velocities and sizes appear `scale` times larger in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// World position of the effect's local origin, in metres.
    pub origin: Vec3,
    /// World metres per local unit; positive.
    pub scale: f32,
}
impl Default for Placement {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            scale: 1.,
        }
    }
}
impl Placement {
    /// The world camera expressed in the effect's local space.
    pub fn local_camera(&self, camera: &CameraSpec) -> Result<CameraSpec> {
        ensure!(
            self.origin.is_finite() && self.scale.is_finite() && self.scale > 0.,
            "invalid VFX placement"
        );
        let local = |p: Vec3| (p - self.origin) / self.scale;
        Ok(match camera {
            CameraSpec::LookAt {
                position,
                target,
                vertical_fov,
            } => CameraSpec::LookAt {
                position: local(*position),
                target: local(*target),
                vertical_fov: *vertical_fov,
            },
            CameraSpec::FrameBounds {
                min,
                max,
                direction,
                padding,
                vertical_fov,
            } => CameraSpec::FrameBounds {
                min: local(*min),
                max: local(*max),
                direction: *direction,
                padding: *padding,
                vertical_fov: *vertical_fov,
            },
            CameraSpec::Orthographic { centre, world_size } => {
                ensure!(
                    self.scale == 1. && self.origin == Vec3::ZERO,
                    "orthographic VFX views cannot be placed"
                );
                CameraSpec::Orthographic {
                    centre: *centre,
                    world_size: *world_size,
                }
            }
        })
    }
}
/// Extracted lighting inputs for lit particles.
pub struct Lighting {
    /// Fallback diffuse incident radiance when probes are absent/uninitialized.
    pub ambient: Vec3,
    /// Normalized direction toward the scene sun.
    pub sun_direction: Vec3,
    /// Linear RGB directional illuminance.
    pub sun_color: Vec3,
    /// Optional completed fidelity probe field; returns in Graphics/ShaderRead.
    pub probes: Option<ProbeLighting>,
}
impl Default for Lighting {
    fn default() -> Self {
        Self {
            ambient: Vec3::new(0.12, 0.17, 0.24),
            sun_direction: Vec3::new(0.4, 1., 0.3).normalize(),
            sun_color: Vec3::new(1.5, 1.3, 1.1),
            probes: None,
        }
    }
}
/// Read-only fidelity DDGI field, borrowed until the next fidelity render.
pub struct ProbeLighting {
    /// Same-device directional-probe storage.
    pub buffer: Buffer,
    /// Published native storage-buffer index.
    pub index: u32,
    /// First lattice position in metres.
    pub origin: Vec3,
    /// Probe separation in metres.
    pub spacing: Vec3,
    /// XYZ lattice dimensions.
    pub counts: [u32; 3],
    /// Actual previous access; ownership must be Graphics.
    pub state: State,
}
/// Imported linear HDR scene color, preserved beneath the VFX draw.
#[derive(Clone, Copy)]
pub struct SceneColor {
    /// Same-device RGBA32-float attachment matching the feature's size.
    pub image: Image,
    /// Published sampled-image index for tone mapping the composited result.
    pub sampled_index: u32,
    /// Actual prior access, owned by Graphics. Returns in Graphics/ShaderRead.
    pub state: State,
}
/// Imported opaque scene depth, shared by soft particles and GPU collisions.
#[derive(Clone, Copy)]
pub struct SceneDepth {
    /// Same-device sampled image containing reversed-Z depth.
    pub image: Image,
    /// Its published native sampled-image heap index.
    pub sampled_index: u32,
    /// Actual prior access; ownership must already belong to Graphics.
    pub state: State,
}
impl Default for View {
    fn default() -> Self {
        Self {
            scene_background: None,
            linear_depth: false,
            lighting: Lighting::default(),
            camera: CameraSpec::LookAt {
                position: Vec3::new(5., 3.4, 6.),
                target: Vec3::new(0., 1.4, 0.),
                vertical_fov: 0.75,
            },
            exposure: 1.,
            async_compute: false,
            scene_depth: None,
            scene_color: None,
            placement: Placement::default(),
            intensity: 1.,
        }
    }
}
pub(crate) struct Tracked {
    pub resource: Resource,
    pub state: State,
}
pub(crate) struct Pipelines {
    pub compute: Vec<Pipeline>,
    pub backdrop: Pipeline,
    pub particles: Pipeline,
    pub tonemap: Pipeline,
}
/// Owned GPU particle resources. The engine retains the device; this can be an ECS
/// render component and registers passes without changing ara-render's scene type.
pub struct GpuSystem {
    pub(crate) effect: Effect,
    pub(crate) layout: Layout,
    pub(crate) resources: Vec<Tracked>,
    pub(crate) particles: Buffer,
    pub(crate) lists: Buffer,
    pub(crate) control: Buffer,
    pub(crate) frame: Buffer,
    pub(crate) read_control: Buffer,
    pub(crate) read_color: Buffer,
    pub(crate) hdr: Image,
    pub(crate) depth: Image,
    pub(crate) output: Image,
    pub(crate) feature_ids: Image,
    pub(crate) read_ids: Buffer,
    pub(crate) material_buffer: Buffer,
    pub(crate) material_index: u32,
    pub(crate) material_textures: Vec<(Image, State)>,
    pub(crate) indices: [u32; 4],
    pub(crate) config_index: u32,
    pub(crate) depth_index: u32,
    pub(crate) atlas_index: u32,
    pub(crate) hdr_index: u32,
    pub(crate) color_target: Image,
    pub(crate) color_index: u32,
    pub(crate) preview_background: bool,
    pub(crate) pipelines: Pipelines,
    pub(crate) size: [u32; 2],
    pub(crate) tick: u32,
    pub(crate) timings: Vec<ara_gpu::commands::PassTiming>,
    pub(crate) metadata: ShaderMetadata,
    pub(crate) hash: String,
    pub(crate) visible: u32,
    pub(crate) async_compute: bool,
    pub(crate) last_frame: Frame,
}
impl GpuSystem {
    /// Completed GPU pass timestamps from the most recent step or render. Reading
    /// these host-side measurements performs no particle-buffer readback.
    pub fn timings(&self) -> &[ara_gpu::commands::PassTiming] {
        &self.timings
    }

    /// Advance one fixed tick and render it through declared graph passes.
    pub fn step(&mut self, gpu: &mut VulkanDevice, view: &View) -> Result<()> {
        self.run_frame(gpu, view, false, true, true)
    }
    /// Advance one fixed tick without drawing it (no backdrop copy, draw or
    /// tonemap). Live composition advances by the elapsed ticks, then calls
    /// [`GpuSystem::render`] once per presented frame.
    pub fn advance(&mut self, gpu: &mut VulkanDevice, view: &View) -> Result<()> {
        self.run_frame(gpu, view, false, true, false)
    }
    /// Replay from tick zero and assemble a standard ara-render capture at time T.
    /// Capture tick is completed fixed ticks; settle frames are bounded and do not advance time.
    pub fn capture(&mut self, gpu: &mut VulkanDevice, request: &CaptureRequest) -> Result<Capture> {
        self.capture_over(gpu, request, View::default())
    }
    /// [`GpuSystem::capture`] composited over a scene: `scene` supplies the background,
    /// depth and lighting of every replayed frame (its camera and exposure come from
    /// `request`). The scene inputs must stay valid and unchanged for the whole replay.
    pub fn capture_over(
        &mut self,
        gpu: &mut VulkanDevice,
        request: &CaptureRequest,
        scene: View,
    ) -> Result<Capture> {
        ensure!(
            request.deterministic && request.seed == u64::from(self.effect.seed),
            "VFX captures require deterministic mode and the authored seed"
        );
        ensure!(
            matches!(request.mode, ara_render::capture::ViewMode::Lit),
            "VFX feature capture currently supports the lit channel"
        );
        ensure!(
            request.size == self.size
                && request.tick > 0
                && request.tick <= 1_000_000
                && request.settle_frames <= 1024,
            "invalid VFX capture extent, tick or settle count"
        );
        self.reset(gpu)?;
        let view = View {
            camera: request.camera,
            exposure: request.exposure,
            async_compute: false,
            ..scene
        };
        for _ in 1..request.tick {
            self.run_frame(gpu, &view, false, true, true)?;
        }
        let settle = request.settle_frames.max(1);
        self.run_frame(gpu, &view, settle == 1, true, true)?;
        for frame in 1..settle {
            self.run_frame(gpu, &view, frame + 1 == settle, false, true)?;
        }
        let rgba = gpu.read(self.read_color)?;
        let ids = gpu
            .read(self.read_ids)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| f32::from_le_bytes([p[0], p[1], p[2], p[3]]) as u32)
            .collect();
        let names = vec![(1, self.effect.name.clone())];
        Capture::from_feature(
            FeatureCapture {
                rgba,
                ids,
                names: &names,
                hash: &self.hash,
                device: &gpu.context().info,
                timings: &self.timings,
                shaders: &self.metadata,
            },
            request,
        )
    }
    pub(crate) fn mesh_diameter(&self, index: usize) -> f32 {
        self.effect.emitters[index]
            .stacks
            .render
            .iter()
            .fold(1., |max, m| match &m.module {
                Module::Mesh { triangles, .. } => triangles
                    .iter()
                    .flatten()
                    .fold(max, |r, v| r.max(2. * Vec3::from(*v).length())),
                _ => max,
            })
    }
    /// Current SDR output image; ownership/layout remain tracked by this feature.
    pub fn output(&self) -> Image {
        self.output
    }
    /// Completed composited HDR image, ready for the fidelity post stack.
    pub fn hdr_output(&self) -> SceneColor {
        SceneColor {
            image: self.color_target,
            sampled_index: self.color_index,
            state: State {
                access: Access::ShaderRead,
                queue: Queue::Graphics,
            },
        }
    }
    /// Redraw a completed tick with new view inputs, without advancing simulation.
    pub fn render(&mut self, gpu: &mut VulkanDevice, view: &View) -> Result<()> {
        ensure!(self.tick > 0, "step before rendering a completed tick");
        self.run_frame(gpu, view, false, false, true)
    }
    /// Bind material-system instance rows and all texture images reachable by them.
    /// Call at a completed frame boundary; textures return in Graphics/ShaderRead.
    /// Packed rows must use the layouts of the materials supplied to cook_source.
    pub fn bind_materials(
        &mut self,
        gpu: &mut VulkanDevice,
        parameters: &[ara_material::PackedParameters],
        textures: &[(Image, State)],
    ) -> Result<()> {
        ensure!(
            parameters.len() == self.effect.emitters.len(),
            "one packed material per emitter"
        );
        ensure!(
            textures.iter().all(|(_, s)| s.queue == Queue::Graphics),
            "material textures must be Graphics-owned"
        );
        let mut rows = vec![[0f32; 4]; 16 * 64];
        for (i, p) in parameters.iter().enumerate() {
            ensure!(
                p.rows.len() <= 64 && p.rows.iter().flatten().all(|v| v.is_finite()),
                "invalid material rows"
            );
            rows[i * 64..i * 64 + p.rows.len()].copy_from_slice(&p.rows);
        }
        gpu.upload_queued(self.material_buffer, bytemuck::cast_slice(&rows))?;
        self.resources[12].state.access = Access::HostWrite;
        self.material_textures = textures.to_vec();
        Ok(())
    }
    /// Explicitly retire feature-owned resources after their submitted work completes.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        for r in self.resources {
            gpu.retire(r.resource)?;
        }
        for pipeline in self.pipelines.compute.into_iter().chain([
            self.pipelines.backdrop,
            self.pipelines.particles,
            self.pipelines.tonemap,
        ]) {
            gpu.retire_pipeline(pipeline)?;
        }
        gpu.collect()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_maps_the_world_camera_into_effect_space() {
        let world = CameraSpec::LookAt {
            position: Vec3::new(14., 3., 10.),
            target: Vec3::new(10., 1., 10.),
            vertical_fov: 0.8,
        };
        let placement = Placement {
            origin: Vec3::new(10., 1., 10.),
            scale: 2.,
        };
        let CameraSpec::LookAt {
            position, target, ..
        } = placement.local_camera(&world).unwrap()
        else {
            panic!("look-at stays look-at");
        };
        assert_eq!((position, target), (Vec3::new(2., 1., 0.), Vec3::ZERO));
        assert!(
            Placement {
                scale: 0.,
                ..placement
            }
            .local_camera(&world)
            .is_err()
        );
        assert_eq!(
            format!("{:?}", Placement::default().local_camera(&world).unwrap()),
            format!("{world:?}")
        );
    }
}
