//! Volumetric fog (Unreal's volumetric fog; Wronski 2014, Hillaire 2015): a near-field
//! froxel volume in front of the coarse 4 km fog grid (`Fog.Froxels`, look-sky.slang),
//! and local lights' scattering per pixel.
//!
//! Each frame `Fog.Inject` (look-volumetric.slang) evaluates every froxel at a jittered
//! point (a Halton sequence over frames since the last history reset): the media
//! (height fog, local fog volumes, emission) and their lighting (the sun through the
//! cascades and far map, probe GI or sky ambient), and blends it into last frame's
//! volume reprojected through the unjittered previous view-projection. `Fog.Integrate`
//! then accumulates in-scatter and transmittance front to back per column (Hillaire's
//! energy-conserving segment integral). Shading composites the near volume up to the
//! fog's volumetric distance (a cubic B-spline across froxels, so shafts never step) and
//! the coarse grid's segment behind it (look.slangh `applyFog`); the coarse grid then
//! starts at that distance.
//!
//! Local lights skip both grids: `Fog.Lights` (look-fog-lights.slang) integrates each
//! light of a pixel's tile along the pixel's own ray, clipped analytically to the light's
//! range, cone or barn doors, by equiangular sampling (Kulla and Fajardo 2012) on fixed
//! angular bins, with shadows filtered over each sample's stretch and the froxels'
//! transmittance: noise-free and deterministic, with no history. It runs at the render
//! size over `VolumetricFogQuality::light_scale` (half: upsampled by depth), and applyFog
//! adds it. With `light_samples` zero (or without the near volume, as on Low) local
//! lights scatter in the froxels as before.
//!
//! Slices sit on an exponential view-depth distribution and are tiled in 2D atlases
//! (the GPU layer has no 3D images): `columns` slices per atlas row. Between-slice
//! filtering is manual; in-slice taps clamp inside each slice's tile.
use super::{
    frame::LookFrame,
    gpu::{
        FROXELS, LookGpu, LookIds, MULTISCATTER, SKY_IRRADIANCE, TRANSMITTANCE, VOLUME_A,
        VOLUME_AMBIENT, VOLUME_B, VOLUME_LIGHT, VOLUME_SCATTER,
    },
    passes::LookPass,
};
use crate::graph::{Graph, PassId, ResourceId, State};
use anyhow::Result;
use ara_gpu::{Access, Device, GpuDevice, Queue, Resource, vk};
use glam::{Mat4, Quat, Vec3};

/// Local fog volumes a frame renders at most (`Renderer::set_fog_volumes`): the
/// first ones given, which hosts sort by priority and distance.
pub const MAX_FOG_VOLUMES: usize = 16;
/// Bytes of one fog volume record in the look buffer (look-fog.slangh `FogVolumeRecord`).
pub(crate) const FOG_VOLUME_BYTES: usize = 96;
const _: () = assert!(std::mem::size_of::<[[f32; 4]; 6]>() == FOG_VOLUME_BYTES);
/// The exponential slice distribution's scale (m): depth(t) = c (2^(t log2(1 + D/c)) - 1),
/// a few centimetres per slice at the camera, metres at the volumetric distance.
const SLICE_SCALE: f32 = 2.;
/// Share of last frame's reprojected volume kept each frame once converged (Unreal's
/// 0.9); the first frames after a reset average evenly instead (deterministic captures
/// converge over their settle frames).
const HISTORY_WEIGHT: f32 = 0.9;
/// The largest atlas side (the device's 2D image limit the layout stays under).
const ATLAS_LIMIT: u32 = 16384;
/// Froxels per ambient light cell across, down and in depth: ambient light (probe GI
/// or the sky) is smooth, and evaluating it per froxel cost half of injection.
const AMBIENT_CELL: [u32; 3] = [4, 4, 2];

/// Volumetric lighting of height fog (Unreal's `bVolumetricFog` settings).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumetricFog {
    /// View depth in metres the near froxel volume covers (Unreal's volumetric fog
    /// view distance); the coarse grid carries the fog beyond it. 1 to 1000.
    pub distance: f32,
    /// Light the height fog emits (linear radiance per metre of fog at its base
    /// density, following the fog's height falloff), as glowing haze.
    pub emissive: [f32; 3],
    /// Scale of the height fog's extinction (not its scattering): above one the fog
    /// absorbs more light, darkening shafts' surroundings. 0.1 to 10.
    pub extinction_scale: f32,
}
impl Default for VolumetricFog {
    fn default() -> Self {
        Self {
            distance: 60.,
            emissive: [0.; 3],
            extinction_scale: 1.,
        }
    }
}
impl VolumetricFog {
    pub(crate) fn valid(&self) -> bool {
        (1.0..=1000.0).contains(&self.distance)
            && (0.1..=10.0).contains(&self.extinction_scale)
            && self.emissive.iter().all(|c| c.is_finite() && *c >= 0.)
    }
}

/// The near volume's budget per preset (`Quality::volumetric_fog`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumetricFogQuality {
    /// Render pixels per froxel side (4 to 32): narrower shafts survive smaller tiles.
    pub tile: u32,
    /// Depth slices up to the volumetric distance (16 to 128).
    pub slices: u32,
    /// Render pixels per side of a texel of the per-pixel local-light scattering
    /// (`Fog.Lights`): 1 at full resolution, 2 at half (upsampled by depth).
    pub light_scale: u32,
    /// Angular bins across the half circle a ray subtends at a local light (2 to 32):
    /// one equiangular sample each, so a ray passing right by a light takes this many
    /// and distant ones fewer. Zero scatters local lights in the froxels instead.
    pub light_samples: u32,
}
impl Default for VolumetricFogQuality {
    /// 8-pixel froxels, 64 slices, local lights at half resolution in 16 bins.
    fn default() -> Self {
        Self {
            tile: 8,
            slices: 64,
            light_scale: 2,
            light_samples: 16,
        }
    }
}
impl VolumetricFogQuality {
    pub(crate) fn valid(&self) -> bool {
        (4..=32).contains(&self.tile)
            && (16..=128).contains(&self.slices)
            && (1..=2).contains(&self.light_scale)
            && (self.light_samples == 0 || (2..=32).contains(&self.light_samples))
    }
}

/// The shape of a local fog volume in its local space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FogVolumeShape {
    /// An ellipsoid inscribed in the half extents.
    #[default]
    Sphere,
    /// A box of the half extents.
    Box,
}

/// A local fog volume (Unreal's Local Fog Volume): a sphere or box of fog placed in
/// the world, rendered in the near froxel volume and the coarse grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FogVolume {
    /// World centre in metres.
    pub centre: [f32; 3],
    /// World rotation (unit quaternion, xyzw).
    pub rotation: [f32; 4],
    /// Half size along the rotated axes, in metres.
    pub half_extent: [f32; 3],
    /// Sphere (ellipsoid) or box.
    pub shape: FogVolumeShape,
    /// Extinction at the centre, per metre (0 to 10).
    pub density: f32,
    /// Exponential falloff with world height above the centre (and rise below it),
    /// per metre; zero is uniform.
    pub height_falloff: f32,
    /// Share of the radius over which density fades to zero at the edge (0 hard,
    /// 1 from the centre outwards).
    pub radial_falloff: f32,
    /// Single-scattering albedo (linear, 0 to 1 per channel).
    pub albedo: [f32; 3],
    /// Emitted linear radiance per metre at the centre's density.
    pub emissive: [f32; 3],
}
impl Default for FogVolume {
    fn default() -> Self {
        Self {
            centre: [0.; 3],
            rotation: [0., 0., 0., 1.],
            half_extent: [5.; 3],
            shape: FogVolumeShape::Sphere,
            density: 0.1,
            height_falloff: 0.,
            radial_falloff: 1.,
            albedo: [0.9; 3],
            emissive: [0.; 3],
        }
    }
}
impl FogVolume {
    pub(crate) fn valid(&self) -> bool {
        let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
        finite(&self.centre)
            && finite(&self.rotation)
            && self.half_extent.iter().all(|x| x.is_finite() && *x >= 0.01)
            && (0.0..=10.0).contains(&self.density)
            && self.height_falloff.is_finite()
            && self.height_falloff.abs() <= 100.
            && (0.0..=1.0).contains(&self.radial_falloff)
            && self.albedo.iter().all(|c| (0.0..=1.0).contains(c))
            && self.emissive.iter().all(|c| c.is_finite() && *c >= 0.)
    }

    /// The shader record: world-to-local rows (the unit sphere or cube), then
    /// density, height falloff, radial falloff and shape; albedo and the centre's
    /// height; emission.
    fn row(&self) -> [[f32; 4]; 6] {
        let rotation = Quat::from_array(self.rotation).normalize();
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from_array(self.half_extent),
            rotation,
            Vec3::from_array(self.centre),
        )
        .inverse()
        .transpose()
        .to_cols_array_2d();
        let [r, g, b] = self.albedo;
        let [er, eg, eb] = self.emissive;
        [
            local[0],
            local[1],
            local[2],
            [
                self.density,
                self.height_falloff,
                self.radial_falloff,
                f32::from(u8::from(self.shape == FogVolumeShape::Box)),
            ],
            [r, g, b, self.centre[1]],
            [er, eg, eb, 0.],
        ]
    }
}

/// Froxel grid and atlas of the near volume for one render size and budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    /// Froxels across, down and in depth.
    pub grid: [u32; 3],
    /// Render pixels per froxel side.
    pub tile: u32,
    /// Slices per atlas row.
    pub columns: u32,
    /// The ambient light grid (`AMBIENT_CELL` froxels per cell).
    pub ambient: [u32; 3],
    /// Its slices per atlas row.
    pub ambient_columns: u32,
    /// The per-pixel local-light scatter (`Fog.Lights`); `None` scatters local lights
    /// in the froxels.
    pub lights: Option<Lights>,
}

/// The per-pixel local-light scatter's image and budget (`Layout::lights`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Lights {
    /// Texels across and down: the render size over `scale`.
    pub size: [u32; 2],
    /// Render pixels per texel side.
    pub scale: u32,
    /// Angular bins per half circle (`VolumetricFogQuality::light_samples`).
    pub samples: u32,
}
impl Layout {
    pub(crate) fn new(render: [u32; 2], quality: VolumetricFogQuality) -> Self {
        let [x, y] = render.map(|v| v.div_ceil(quality.tile).max(1));
        let grid = [x, y, quality.slices];
        let ambient = [0, 1, 2].map(|i| grid[i].div_ceil(AMBIENT_CELL[i]));
        Self {
            grid,
            tile: quality.tile,
            columns: (ATLAS_LIMIT / x).clamp(1, quality.slices),
            ambient,
            ambient_columns: (ATLAS_LIMIT / ambient[0]).clamp(1, ambient[2]),
            lights: (quality.light_samples > 0).then(|| Lights {
                size: render.map(|v| v.div_ceil(quality.light_scale).max(1)),
                scale: quality.light_scale,
                samples: quality.light_samples,
            }),
        }
    }

    /// The atlas size: `columns` slices across, as many rows as the rest need.
    pub(crate) fn atlas(&self) -> [u32; 2] {
        atlas(self.grid, self.columns)
    }

    /// The ambient light atlas size.
    pub(crate) fn ambient_atlas(&self) -> [u32; 2] {
        atlas(self.ambient, self.ambient_columns)
    }
}

/// A slice atlas of `grid` cells, `columns` slices per row.
fn atlas([x, y, z]: [u32; 3], columns: u32) -> [u32; 2] {
    [x * columns, y * z.div_ceil(columns)]
}

impl LookGpu {
    /// Whether the look asks for the near volume: volumetric height fog or any local
    /// fog volume.
    pub(crate) fn wants_volumetric(&self) -> bool {
        self.look.fog.is_some_and(|f| f.volumetric.is_some()) || !self.fog_volumes.is_empty()
    }

    /// Whether the coarse fog grid runs: a physical sky, height fog or a fog volume.
    pub(crate) fn fogged(&self) -> bool {
        self.look.sky.is_some() || self.look.fog.is_some() || !self.fog_volumes.is_empty()
    }

    /// Allocates (or reallocates, after a resize or budget change) the near volume's
    /// injection history pair, its integrated atlas and the per-pixel local-light
    /// scatter while the look and quality ask for them. Never frees otherwise.
    pub(crate) fn ensure_volumetric(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        let Some(quality) = self
            .quality
            .volumetric_fog
            .filter(|_| self.wants_volumetric())
        else {
            return Ok(());
        };
        let layout = Layout::new(self.render, quality);
        if self.volumetric == Some(layout) {
            return Ok(());
        }
        for i in [
            VOLUME_A,
            VOLUME_B,
            VOLUME_LIGHT,
            VOLUME_AMBIENT,
            VOLUME_SCATTER,
        ] {
            if let Some(old) = self.images[i].take() {
                gpu.retire(Resource::Image(old))?;
            }
            let [width, height] = match i {
                VOLUME_AMBIENT => layout.ambient_atlas(),
                VOLUME_SCATTER => match layout.lights {
                    Some(lights) => lights.size,
                    None => continue,
                },
                _ => layout.atlas(),
            };
            let image = gpu.create_image(
                width,
                height,
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
            )?;
            self.sampled[i] = gpu.sampled_image(image, false)?.index;
            self.storage[i] = gpu.storage_image(image)?.index;
            self.states[i] = State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            };
            self.images[i] = Some(image);
        }
        self.volumetric = Some(layout);
        self.volumetric_frames = 0;
        Ok(())
    }

    /// Decides whether this frame renders the near volume (`frame_volumetric`):
    /// `eligible` (a lit perspective frame) with the volume allocated. `reset`
    /// (temporal history restarting) drops the reprojected history; otherwise the
    /// history pair swaps.
    pub(crate) fn begin_volumetric(&mut self, eligible: bool, reset: bool) {
        self.frame_volumetric = eligible
            && self.wants_volumetric()
            && self.volumetric.is_some()
            && self.quality.volumetric_fog.is_some();
        if !self.frame_volumetric {
            // History left behind is stale by the next volumetric frame.
            self.volumetric_frames = 0;
            return;
        }
        if reset {
            self.volumetric_frames = 0;
        }
        self.volumetric_slot ^= 1;
    }

    /// This frame's per-pixel local-light scatter (`Fog.Lights`): on near volume frames
    /// whose budget scatters local lights per pixel.
    pub(crate) fn frame_fog_lights(&self) -> Option<Lights> {
        self.volumetric
            .filter(|_| self.frame_volumetric)
            .and_then(|layout| layout.lights)
    }

    /// The images this frame injects into and reprojects from.
    fn volume_images(&self) -> (usize, usize) {
        if self.volumetric_slot == 0 {
            (VOLUME_A, VOLUME_B)
        } else {
            (VOLUME_B, VOLUME_A)
        }
    }

    /// Writes the fog volume records (the coarse grid reads them too) and, on a near
    /// volume frame, its rows and flag; counts the frame towards convergence.
    pub(crate) fn volumetric_rows(&mut self, frame: &mut LookFrame) {
        for (row, volume) in frame.fog_volumes.iter_mut().zip(&self.fog_volumes) {
            *row = volume.row();
        }
        frame.volumetric_atlas[2] = self.fog_volumes.len() as u32;
        frame.volumetric_atlas[3] = std::mem::offset_of!(LookFrame, fog_volumes) as u32;
        let Some(layout) = self.volumetric.filter(|_| self.frame_volumetric) else {
            return;
        };
        let n = self.volumetric_frames;
        let history = if n == 0 {
            0.
        } else {
            (n as f32 / (n as f32 + 1.)).min(HISTORY_WEIGHT)
        };
        let distance = self
            .look
            .fog
            .and_then(|f| f.volumetric)
            .unwrap_or_default()
            .distance;
        frame.flags[1] |= super::frame::VOLUMETRIC;
        frame.volumetric[0] = distance;
        frame.volumetric[1] = SLICE_SCALE;
        frame.volumetric[2] = history;
        let [x, y, z] = layout.grid;
        frame.volumetric_grid = [x, y, z, layout.tile];
        frame.volumetric_atlas[0] = layout.columns;
        frame.volumetric_atlas[1] = n;
        let (current, previous) = self.volume_images();
        frame.volumetric_images = [
            self.sampled[previous],
            self.sampled[current],
            self.sampled[VOLUME_LIGHT],
            self.storage[current],
        ];
        frame.volumetric_storage = [
            self.storage[VOLUME_LIGHT],
            self.storage[VOLUME_AMBIENT],
            self.sampled[VOLUME_AMBIENT],
            0,
        ];
        let [ax, ay, az] = layout.ambient;
        frame.volumetric_ambient = [ax, ay, az, layout.ambient_columns];
        if let Some(lights) = layout.lights {
            frame.fog_lights_params = [lights.samples as f32, 0., 0., 0.];
            frame.fog_lights_info = [
                self.sampled[VOLUME_SCATTER],
                self.storage[VOLUME_SCATTER],
                lights.scale,
                lights.size[0],
            ];
        }
        self.volumetric_frames = n.saturating_add(1);
    }

    /// What the fog's lighting samples beyond the shadow maps (`shared` in the passes):
    /// the physical sky's LUTs and irradiance, and the probe pool's metadata, moments
    /// and this frame's irradiance (look-fog.slangh `fogAmbient`).
    pub(crate) fn fog_light_reads(&self, ids: &LookIds) -> Vec<(ResourceId, Access)> {
        let mut reads = Vec::new();
        if self.look.sky.is_some() {
            reads.extend(
                [TRANSMITTANCE, MULTISCATTER, SKY_IRRADIANCE]
                    .map(|i| (ids.image(i), Access::ShaderRead)),
            );
        }
        if let Some(gi) = ids.gi {
            reads.extend([1, 2, 4].map(|i| (gi[i], Access::ShaderRead)));
        }
        reads
    }

    /// Declares the coarse fog grid (`Fog.Froxels`: 64x36 columns to 4 km, starting
    /// behind the near volume on its frames) after the probe relight it reads.
    /// `shared` is the frame's conservative bindless reachability.
    pub(crate) fn fog_pass(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        shared: &[(ResourceId, Access)],
    ) -> Option<(PassId, LookPass)> {
        if !self.fogged() {
            return None;
        }
        let id = graph.pass(
            "Fog.Froxels",
            Queue::Graphics,
            [
                (ids.image(FROXELS), Access::Storage),
                (ids.frame, Access::ShaderRead),
            ]
            .into_iter()
            .chain(self.fog_light_reads(ids))
            .chain(shared.iter().copied()),
        );
        Some((id, LookPass::Fog))
    }

    /// Declares `Fog.Ambient`, `Fog.Inject` and `Fog.Integrate` on near volume frames,
    /// then `Fog.Lights` where the budget scatters local lights per pixel, before
    /// lighting. `reads` must cover what the fog's lighting samples: the sun and local
    /// shadow maps, the horizon, probe GI and the sky's irradiance; `pixels` what the
    /// per-pixel pass adds: the depth target and the tiled light lists (after
    /// `TiledLightCull`), and the coarse grid (its transmittance) is read here.
    pub(crate) fn volumetric_passes(
        &self,
        graph: &mut Graph,
        ids: &LookIds,
        reads: &[(ResourceId, Access)],
        pixels: &[(ResourceId, Access)],
    ) -> Vec<(PassId, LookPass)> {
        if !self.frame_volumetric {
            return Vec::new();
        }
        let (current, previous) = self.volume_images();
        // The first frame after a reset has no history to read (nor a producer for it).
        let history =
            (self.volumetric_frames > 1).then(|| (ids.image(previous), Access::ShaderRead));
        let ambient = graph.pass(
            "Fog.Ambient",
            Queue::Graphics,
            [
                (ids.image(VOLUME_AMBIENT), Access::Storage),
                (ids.frame, Access::ShaderRead),
            ]
            .into_iter()
            .chain(reads.iter().copied()),
        );
        let inject = graph.pass(
            "Fog.Inject",
            Queue::Graphics,
            [
                (ids.image(VOLUME_AMBIENT), Access::ShaderRead),
                (ids.image(current), Access::Storage),
                (ids.frame, Access::ShaderRead),
            ]
            .into_iter()
            .chain(history)
            .chain(reads.iter().copied()),
        );
        let integrate = graph.pass(
            "Fog.Integrate",
            Queue::Graphics,
            [
                (ids.image(current), Access::ShaderRead),
                (ids.image(VOLUME_LIGHT), Access::Storage),
                (ids.frame, Access::ShaderRead),
            ]
            .into_iter()
            .chain(reads.iter().copied()),
        );
        let mut passes = vec![
            (ambient, LookPass::FogAmbient),
            (inject, LookPass::FogInject),
            (integrate, LookPass::FogIntegrate),
        ];
        if self.frame_fog_lights().is_some() {
            let lights = graph.pass(
                "Fog.Lights",
                Queue::Graphics,
                [
                    (ids.image(VOLUME_LIGHT), Access::ShaderRead),
                    (ids.image(FROXELS), Access::ShaderRead),
                    (ids.image(VOLUME_SCATTER), Access::Storage),
                    (ids.frame, Access::ShaderRead),
                ]
                .into_iter()
                .chain(pixels.iter().copied())
                .chain(reads.iter().copied()),
            );
            passes.push((lights, LookPass::FogLights));
        }
        passes
    }

    /// The fog images shading samples: the coarse grid, and the near volume and the
    /// per-pixel local-light scatter on frames that render them.
    pub(crate) fn fog_reads(&self, ids: &LookIds) -> Vec<(ResourceId, Access)> {
        let mut reads = Vec::new();
        if self.fogged() {
            reads.push((ids.image(FROXELS), Access::ShaderRead));
        }
        if self.frame_volumetric {
            reads.push((ids.image(VOLUME_LIGHT), Access::ShaderRead));
        }
        if self.frame_fog_lights().is_some() {
            reads.push((ids.image(VOLUME_SCATTER), Access::ShaderRead));
        }
        reads
    }

    /// Dispatch size of a near volume pass: one thread per ambient cell, froxel
    /// (injection) or local-light scatter texel, or per column (integration), 8x8 per
    /// group.
    pub(crate) fn volumetric_dispatch(&self, pass: LookPass) -> [u32; 3] {
        let Some(layout) = self.volumetric else {
            return [0; 3];
        };
        let [x, y, z] = match pass {
            LookPass::FogAmbient => layout.ambient,
            LookPass::FogInject => layout.grid,
            LookPass::FogLights => match layout.lights {
                Some(Lights { size: [x, y], .. }) => [x, y, 1],
                None => [0; 3],
            },
            _ => [layout.grid[0], layout.grid[1], 1],
        };
        [x.div_ceil(8), y.div_ceil(8), z]
    }

    /// Replaces the local fog volumes (at most [`MAX_FOG_VOLUMES`], the first kept).
    pub(crate) fn set_fog_volumes(&mut self, volumes: &[FogVolume]) -> Result<()> {
        anyhow::ensure!(
            volumes.iter().all(FogVolume::valid),
            "Invalid fog volume: finite pose, half extents of at least 1 cm, density 0-10, \
             radial falloff 0-1 and albedo 0-1"
        );
        self.fog_volumes.clear();
        self.fog_volumes
            .extend(volumes.iter().take(MAX_FOG_VOLUMES).copied());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_layouts_stay_inside_the_image_limit() {
        for (render, tile, slices) in [
            ([2560, 1440], 8, 96),
            ([3840, 2160], 4, 128),
            ([1707, 960], 12, 64),
            ([640, 360], 16, 48),
        ] {
            let layout = Layout::new(
                render,
                VolumetricFogQuality {
                    tile,
                    slices,
                    ..VolumetricFogQuality::default()
                },
            );
            let [w, h] = layout.atlas();
            assert!(w <= ATLAS_LIMIT && h <= ATLAS_LIMIT, "{render:?} {w}x{h}");
            // Every slice has a tile, and the grid covers the render size.
            assert!(layout.columns * h / layout.grid[1] >= slices);
            assert!(layout.grid[0] * tile >= render[0] && layout.grid[1] * tile >= render[1]);
        }
    }

    #[test]
    fn fog_volume_record_maps_the_volume_onto_the_unit_shape() {
        let volume = FogVolume {
            centre: [10., 2., -4.],
            half_extent: [2., 1., 4.],
            ..FogVolume::default()
        };
        let row = volume.row();
        let local = |p: [f32; 3]| {
            let p = [p[0], p[1], p[2], 1.];
            [0, 1, 2].map(|r| (0..4).map(|c| row[r][c] * p[c]).sum::<f32>())
        };
        assert_eq!(local([10., 2., -4.]), [0., 0., 0.]);
        assert_eq!(local([12., 3., 0.]), [1., 1., 1.]);
        assert_eq!(row[4][3], 2.);
    }
}
