//! Renderer input snapshots, independent of the engine's authoritative ECS state.
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

/// `Material.flags[1]`: decode base-color RGB after raw UNORM filtering.
/// Alpha stays raw; linear factors multiply after this exact sRGB conversion.
pub const BASE_COLOR_DECODE_SRGB: u32 = 1 << 25;
/// `Material.flags[1]`: decode emissive RGB after raw UNORM filtering.
/// Source glTF textures use hardware sRGB views and leave this bit clear.
pub const EMISSIVE_DECODE_SRGB: u32 = 1 << 26;
/// `Material.flags[1]`: unpublished model bounds. Blended loading bounds draw
/// short screen-space corner marks; opaque missing/broken bounds stay solid.
pub const MODEL_BOUNDS: u32 = 1 << 27;
/// `Material.flags[1]`: glTF `doubleSided`. The native G-buffer lights a back face's
/// own side (its normal reversed, as glTF requires); single-sided materials keep
/// their normals.
pub const DOUBLE_SIDED: u32 = 1 << 28;

/// Interleaved position/normal/UV/tangent vertex ABI, 48 bytes.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// World-space position in metres.
    pub position: [f32; 3],
    /// World-space unit normal.
    pub normal: [f32; 3],
    /// First texture coordinate.
    pub uv: [f32; 2],
    /// Tangent xyz and bitangent handedness w.
    pub tangent: [f32; 4],
}
/// Material parameter block; extraction lowers scene texture indices to bindless slots.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Material {
    /// Linear base color and opacity.
    pub base_color: [f32; 4],
    /// Linear emissive radiance (cd/m²) and perceptual roughness.
    pub emissive_roughness: [f32; 4],
    /// Metallic, normal scale, reserved and alpha cutoff.
    pub surface: [f32; 4],
    /// Scene texture indices: base color (sRGB), normal, metallic/roughness (G/B), emissive (sRGB).
    pub textures: [u32; 4],
    /// Word 0: alpha mode (opaque/mask/blend). Word 1: layer/AO bits 0..24 and
    /// decode-after-sampling bits 25/26, unpublished model bounds bit 27 and
    /// double-sided bit 28.
    /// Word 3 is reserved for the fidelity tag.
    pub flags: [u32; 4],
}
impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: [1.; 4],
            emissive_roughness: [0., 0., 0., 0.5],
            surface: [0., 1., 1., 0.5],
            textures: [0, 1, 0, 0],
            flags: [0; 4],
        }
    }
}
/// Scene geometry with a stable external entity identity.
pub struct Mesh {
    /// Stable document/ECS identity, preserved in capture legends.
    pub entity: u64,
    /// Human-readable capture label.
    pub name: String,
    /// World-space mesh vertices, prepared by scene extraction.
    pub vertices: Vec<Vertex>,
    /// Triangle-list U32 indices.
    pub indices: Vec<u32>,
    /// Index into the scene material list.
    pub material: usize,
}
/// Point/spot light with physical output units.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Light {
    /// World position in metres and finite influence radius in metres.
    pub position_range: [f32; 4],
    /// Linear RGB tint and luminous flux in lumens.
    pub color_lumens: [f32; 4],
    /// Spot forward direction xyz and cosine of outer cone half-angle.
    pub direction_outer: [f32; 4],
    /// Type (0 point, 1 spot), inner cone cosine, source radius in metres (the
    /// near field softens inside it; zero for an ideal point), and w: one keeps
    /// the light out of fog in-scattering (emitter lights), zero includes it.
    pub parameters: [f32; 4],
}

/// Local lights a scene may hold (analytic lights plus emissive surfaces as
/// lights); each 16x16 screen tile shades at most [`TILE_LIGHTS`] of them.
pub const MAX_LOCAL_LIGHTS: usize = 256;
/// Local lights one 16x16 screen tile shades (tiled culling keeps the first).
pub const TILE_LIGHTS: usize = 128;
/// CPU pixels uploaded at scene creation.
pub struct Texture {
    /// Pixel width and height.
    pub size: [u32; 2],
    /// Tightly packed RGBA8 source pixels.
    pub rgba: Vec<u8>,
    /// Decode color from sRGB; scalar/normal data stays linear.
    pub srgb: bool,
}
/// Immutable render extraction snapshot. Gameplay state remains in ECS components.
pub struct Scene {
    /// Opaque and transparent meshes.
    pub meshes: Vec<Mesh>,
    /// Material parameter blocks.
    pub materials: Vec<Material>,
    /// Bindless image inputs. Indices 0 and 1 conventionally contain white and flat normal.
    pub textures: Vec<Texture>,
    /// At most 64 tiled local lights in the baseline.
    pub lights: Vec<Light>,
    /// Direction toward the directional light.
    pub sun_direction: Vec3,
    /// Linear RGB illuminance in lux.
    pub sun_lux: Vec3,
    /// Equirectangular linear HDR environment, interpreted as radiance in cd/m².
    pub environment: Vec<[f32; 4]>,
    /// Environment dimensions.
    pub environment_size: [u32; 2],
}
/// Capture camera specification, serializable for agent requests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CameraSpec {
    /// Axis-aligned orthographic XY view, for 2D and editor projections.
    Orthographic {
        /// World-space view centre.
        centre: glam::Vec2,
        /// Visible world width and height in metres.
        world_size: glam::Vec2,
    },
    /// Perspective camera looking at an arbitrary point.
    LookAt {
        /// Camera position in metres.
        position: Vec3,
        /// Aim point in metres.
        target: Vec3,
        /// Vertical field of view in radians.
        vertical_fov: f32,
    },
    /// Fit a world-space axis-aligned bounds from a chosen direction.
    FrameBounds {
        /// Minimum bounds corner.
        min: Vec3,
        /// Maximum bounds corner.
        max: Vec3,
        /// Direction from bounds centre toward camera.
        direction: Vec3,
        /// Fractional padding; 0.2 adds 20 percent.
        padding: f32,
        /// Vertical field of view in radians.
        vertical_fov: f32,
    },
}
/// Resolved camera with a stable reversed-Z infinite projection.
pub struct Camera {
    /// World-space eye position.
    pub position: Vec3,
    /// Unit viewing direction.
    pub forward: Vec3,
    /// View matrix.
    pub view: Mat4,
    /// Infinite-far reversed-Z projection (near 0.1 m).
    pub projection: Mat4,
}
impl CameraSpec {
    /// Resolve a request using the target's actual aspect ratio.
    pub fn resolve(&self, size: [u32; 2]) -> anyhow::Result<Camera> {
        anyhow::ensure!(!size.contains(&0), "Camera extent must be nonzero");
        let aspect = size[0] as f32 / size[1] as f32;
        if let Self::Orthographic { centre, world_size } = *self {
            anyhow::ensure!(
                centre.is_finite() && world_size.is_finite() && world_size.min_element() > 0.,
                "Invalid orthographic camera"
            );
            let position = centre.extend(1.);
            return Ok(Camera {
                position,
                forward: -Vec3::Z,
                view: Mat4::look_at_rh(position, centre.extend(0.), Vec3::Y),
                projection: Mat4::orthographic_rh(
                    -world_size.x * 0.5,
                    world_size.x * 0.5,
                    -world_size.y * 0.5,
                    world_size.y * 0.5,
                    100.,
                    0.1,
                ),
            });
        }
        let (position, target, fov) = match *self {
            Self::Orthographic { .. } => unreachable!("orthographic request resolved above"),
            Self::LookAt {
                position,
                target,
                vertical_fov,
            } => (position, target, vertical_fov),
            Self::FrameBounds {
                min,
                max,
                direction,
                padding,
                vertical_fov,
            } => {
                anyhow::ensure!(
                    min.is_finite()
                        && max.is_finite()
                        && min.cmple(max).all()
                        && padding >= 0.
                        && padding.is_finite(),
                    "Invalid bounds"
                );
                let direction = direction
                    .try_normalize()
                    .ok_or_else(|| anyhow::anyhow!("Camera direction is zero"))?;
                let centre = (min + max) * 0.5;
                let radius = (max - min).length() * 0.5;
                let half_fov =
                    (vertical_fov * 0.5).min(((vertical_fov * 0.5).tan() * aspect).atan());
                let distance = (radius * (1. + padding) / half_fov.sin()).max(0.2);
                (centre + direction * distance, centre, vertical_fov)
            }
        };
        anyhow::ensure!(
            position.is_finite() && target.is_finite() && fov > 0.01 && fov < 3.13,
            "Invalid perspective camera"
        );
        let forward = (target - position)
            .try_normalize()
            .ok_or_else(|| anyhow::anyhow!("Camera target equals position"))?;
        let up = if forward.dot(Vec3::Y).abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        Ok(Camera {
            position,
            forward,
            view: Mat4::look_at_rh(position, target, up),
            projection: Mat4::perspective_infinite_reverse_rh(fov, aspect, 0.1),
        })
    }
}

/// Generate a smooth UV sphere for material verification.
pub fn sphere(entity: u64, name: String, centre: Vec3, radius: f32, material: usize) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    const X: u32 = 48;
    const Y: u32 = 24;
    for y in 0..=Y {
        for x in 0..=X {
            let u = x as f32 / X as f32;
            let v = y as f32 / Y as f32;
            let theta = u * std::f32::consts::TAU;
            let phi = v * std::f32::consts::PI;
            let normal = Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin());
            vertices.push(Vertex {
                position: (centre + normal * radius).to_array(),
                normal: normal.to_array(),
                uv: [u, v],
                tangent: [-theta.sin(), 0., theta.cos(), 1.],
            });
        }
    }
    for y in 0..Y {
        for x in 0..X {
            let a = y * (X + 1) + x;
            let b = a + X + 1;
            indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    Mesh {
        entity,
        name,
        vertices,
        indices,
        material,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framed_bounds_fit_portrait_and_landscape() {
        for size in [[800, 300], [300, 800]] {
            let c = CameraSpec::FrameBounds {
                min: Vec3::splat(-1.),
                max: Vec3::ONE,
                direction: Vec3::new(1., 1., 1.),
                padding: 0.2,
                vertical_fov: 1.,
            }
            .resolve(size)
            .unwrap();
            for x in [-1., 1.] {
                for y in [-1., 1.] {
                    for z in [-1., 1.] {
                        let p = c.projection * c.view * Vec3::new(x, y, z).extend(1.);
                        assert!(p.x.abs() < p.w && p.y.abs() < p.w);
                    }
                }
            }
        }
    }
}
