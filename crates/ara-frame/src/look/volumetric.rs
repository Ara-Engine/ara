//! Fog volumes and the volumetric-fog budgets. The froxel passes need a renderer.
use glam::{Mat4, Quat, Vec3};

/// Local fog volumes a frame renders at most (`Renderer::set_fog_volumes`): the
/// first ones given, which hosts sort by priority and distance.
pub const MAX_FOG_VOLUMES: usize = 16;
/// Bytes of one fog volume record in the look buffer (look-fog.slangh `FogVolumeRecord`).
pub(crate) const FOG_VOLUME_BYTES: usize = 96;
const _: () = assert!(std::mem::size_of::<[[f32; 4]; 6]>() == FOG_VOLUME_BYTES);
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
    /// (`Fog.Lights`): 1 at full resolution, 2 at half (resolved by depth, its edges
    /// integrated again at full resolution).
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
    /// Whether the volume's numbers are inside the range the shaders store.
    pub fn valid(&self) -> bool {
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
    pub fn row(&self) -> [[f32; 4]; 6] {
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
