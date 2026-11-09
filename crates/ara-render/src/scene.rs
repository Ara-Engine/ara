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
/// `Material.flags[1]`: base colour and opacity multiply by the vertex colour
/// (glTF `COLOR_0`, per-instance paint). Without it vertex colours stay data
/// that only vertex-paint layers and debug views read.
pub const VERTEX_COLOR_TINT: u32 = 1 << 29;
/// `Material.flags[1]`: a vertex-paint layered material (Unreal's VertexColor
/// lerps between material layers). Native `MaterialExtras::terrain_layers` counts
/// its layers and `layer_materials` names them, as for terrain splats, but each
/// layer's weight comes from the vertex colour: `flags[2]` holds one byte per
/// layer (layer `i` in bits `8i..8i+8`), two bits per channel R, G, B, A
/// ([`PaintWeight`]); the weight is the product of the used channels' terms.
pub const VERTEX_PAINT_LAYERS: u32 = 1 << 30;

/// How one vertex-colour channel scales a paint layer's weight
/// ([`VERTEX_PAINT_LAYERS`]): the material graph's lerp tree, flattened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum PaintWeight {
    /// The channel does not affect this layer.
    #[default]
    Unused = 0,
    /// Weight times the channel value `c` (the lerp's second input).
    Channel = 1,
    /// Weight times `1 - c` (the lerp's first input).
    Inverse = 2,
}

impl PaintWeight {
    /// Packs one layer's four channel terms (R, G, B, A) into its `flags[2]` byte.
    pub fn pack(channels: [Self; 4]) -> u8 {
        channels
            .iter()
            .enumerate()
            .fold(0, |byte, (i, term)| byte | ((*term as u8) << (2 * i)))
    }
    /// The weight of a layer byte at linear vertex colour `rgba`, as the shader
    /// evaluates it.
    pub fn weight(byte: u8, rgba: [f32; 4]) -> f32 {
        (0..4).fold(1.0, |w, i| match (byte >> (2 * i)) & 3 {
            1 => w * rgba[i],
            2 => w * (1.0 - rgba[i]),
            _ => w,
        })
    }
}

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
    /// decode-after-sampling bits 25/26, unpublished model bounds bit 27,
    /// double-sided bit 28, vertex-colour tint bit 29 and paint layers bit 30.
    /// Word 2: paint layer weights ([`VERTEX_PAINT_LAYERS`]) on native content.
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
/// A local light with physical output units (`LIGHT_BYTES`; shaders/look-lights.slangh):
/// an ideal point or spot, a sphere or tube source (a point or spot with a source
/// radius, and a length), a rect with barn doors or a disc. Area sources emit uniform
/// luminance: the intensity along their axis over their projected area. Fields beyond
/// the first four default to zero (`..Light::zeroed()`): no shape, no tables.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Light {
    /// World position (a shape's centre) in metres and finite influence radius in
    /// metres, measured from it.
    pub position_range: [f32; 4],
    /// Linear RGB tint and point-equivalent lumens: 4 pi times the luminous intensity
    /// in candela, along `direction_outer` for rects and discs, in every direction for
    /// points and spots (their luminous flux; a spot's cone masks it, as in the path
    /// tracer). Far-field consumers treat every kind as a point of this flux.
    pub color_lumens: [f32; 4],
    /// Emission axis xyz (a spot's cone, a rect's or disc's normal, a light profile's
    /// nadir) and the cosine of a spot's outer cone half-angle (zero for other kinds).
    pub direction_outer: [f32; 4],
    /// x the kind ([`LIGHT_POINT`], [`LIGHT_SPOT`], [`LIGHT_RECT`], [`LIGHT_DISC`]),
    /// plus [`LIGHT_SHAPED`] when a point or spot has a light profile; y a spot's inner
    /// cone cosine; z the source radius in metres (sphere or tube radius, disc radius;
    /// zero for an ideal point); w its volumetric fog scattering intensity (Unreal's;
    /// 1 physical, 0 keeps it out of fog, as emitter lights are).
    pub parameters: [f32; 4],
    /// Unit width axis xyz (a rect's width, a tube's axis, a profile's zero azimuth)
    /// and the tube's length in metres (zero: a sphere).
    pub tangent_length: [f32; 4],
    /// A rect's half width and half height in metres, then where its barn doors' tips
    /// lie: outward of the edges and along the normal, in metres (zero: no doors).
    pub rect: [f32; 4],
    /// Light-table offsets in words (`Renderer::set_light_tables`; zero none): x the
    /// light profile; y, z, w reserved.
    pub tables: [u32; 4],
}

impl Light {
    /// Every field zero: an ideal point light, no shape and no tables. Literals spell
    /// the fields they use and take the rest from it (`..Light::ZERO`).
    pub const ZERO: Self = Self {
        position_range: [0.; 4],
        color_lumens: [0.; 4],
        direction_outer: [0.; 4],
        parameters: [0.; 4],
        tangent_length: [0.; 4],
        rect: [0.; 4],
        tables: [0; 4],
    };

    /// Whether the record is well formed: finite, a positive range and non-negative
    /// output, a known kind, unit axes where the kind uses them (a spot's cone, a
    /// rect's or disc's normal and width axis; a spot's axis is any non-zero vector),
    /// a valid cone, non-negative sizes and fog scattering intensity.
    pub fn is_valid(&self) -> bool {
        let unit = |v: &[f32]| (glam::Vec3::from_slice(v).length() - 1.).abs() < 1e-3;
        let kind = self.parameters[0];
        let base = kind % LIGHT_SHAPED;
        let finite = self
            .position_range
            .iter()
            .chain(&self.color_lumens)
            .chain(&self.direction_outer)
            .chain(&self.parameters)
            .chain(&self.tangent_length)
            .chain(&self.rect)
            .all(|v| v.is_finite());
        let cone = || {
            glam::Vec3::from_slice(&self.direction_outer[..3]).length_squared() > 1e-8
                && (-1.0..=1.0).contains(&self.direction_outer[3])
                && (self.direction_outer[3]..=1.0).contains(&self.parameters[1])
        };
        finite
            && self.position_range[3] > 0.
            && self.color_lumens.iter().all(|v| *v >= 0.)
            && kind.fract() == 0.
            && (0. ..LIGHT_SHAPED * 2.).contains(&kind)
            && (kind < LIGHT_SHAPED || base <= LIGHT_SPOT)
            && self.parameters[2] >= 0.
            && self.parameters[3] >= 0.
            && self.tangent_length[3] >= 0.
            && self.rect.iter().all(|v| *v >= 0.)
            && if base == LIGHT_SPOT {
                cone()
            } else if base >= LIGHT_RECT {
                unit(&self.direction_outer[..3]) && unit(&self.tangent_length[..3])
            } else {
                self.tangent_length[3] == 0. || unit(&self.tangent_length[..3])
            }
    }
}
const _: () = assert!(std::mem::size_of::<Light>() == 112);

/// [`Light::parameters`] x for an ideal or sphere/tube point light.
pub const LIGHT_POINT: f32 = 0.;
/// A spot light (its cone in `direction_outer` and `parameters.y`).
pub const LIGHT_SPOT: f32 = 1.;
/// A one-sided rect light (half extents and barn doors in `rect`).
pub const LIGHT_RECT: f32 = 2.;
/// A one-sided disc light (radius in `parameters.z`).
pub const LIGHT_DISC: f32 = 3.;
/// Added to a point's or spot's kind when its shape row matters to shading (a profile).
pub const LIGHT_SHAPED: f32 = 4.;

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
    /// Orthographic editor view with an arbitrary orientation and target aspect.
    OrthographicLookAt {
        /// Camera position in metres.
        position: Vec3,
        /// Aim point in metres.
        target: Vec3,
        /// Visible vertical span in metres; horizontal span follows the target aspect.
        vertical_size: f32,
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
/// Resolved camera with a stable reversed-Z projection.
pub struct Camera {
    /// World-space eye position.
    pub position: Vec3,
    /// Unit viewing direction.
    pub forward: Vec3,
    /// View matrix.
    pub view: Mat4,
    /// Reversed-Z projection; perspective uses an infinite far plane.
    pub projection: Mat4,
}
impl CameraSpec {
    /// Near-plane distance of oriented orthographic requests, in metres.
    pub const ORTHOGRAPHIC_NEAR: f32 = 0.01;
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
        if let Self::OrthographicLookAt {
            position,
            target,
            vertical_size,
        } = *self
        {
            anyhow::ensure!(
                position.is_finite()
                    && target.is_finite()
                    && vertical_size.is_finite()
                    && vertical_size > 0.
                    && (vertical_size * aspect).is_finite(),
                "Invalid oriented orthographic camera"
            );
            let forward = (target - position)
                .try_normalize()
                .ok_or_else(|| anyhow::anyhow!("Camera target equals position"))?;
            let up = if forward.dot(Vec3::Y).abs() > 0.99 {
                Vec3::Z
            } else {
                Vec3::Y
            };
            return Ok(Camera {
                position,
                forward,
                view: Mat4::look_at_rh(position, target, up),
                projection: Mat4::orthographic_rh(
                    -vertical_size * aspect * 0.5,
                    vertical_size * aspect * 0.5,
                    -vertical_size * 0.5,
                    vertical_size * 0.5,
                    1_000_000.,
                    Self::ORTHOGRAPHIC_NEAR,
                ),
            });
        }
        let (position, target, fov) = match *self {
            Self::Orthographic { .. } | Self::OrthographicLookAt { .. } => {
                unreachable!("orthographic request resolved above")
            }
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
    fn oriented_orthographic_is_parallel_at_any_axis_and_target_aspect() {
        for position in [
            Vec3::new(0., 0., 10.),
            Vec3::new(10., 0., 0.),
            Vec3::new(0., 10., 0.),
            Vec3::new(7., 4., 9.),
        ] {
            for size in [[1200, 600], [600, 1200]] {
                let camera = CameraSpec::OrthographicLookAt {
                    position,
                    target: Vec3::ZERO,
                    vertical_size: 4.,
                }
                .resolve(size)
                .expect("valid orthographic view");
                let right = camera.view.inverse().x_axis.truncate();
                let matrix = camera.projection * camera.view;
                let first = matrix * right.extend(1.);
                let deeper = matrix * (right + camera.forward * 10.).extend(1.);
                let expected = 2. / (4. * size[0] as f32 / size[1] as f32);
                assert!((first.x / first.w - expected).abs() < 1e-5);
                assert!((first.x / first.w - deeper.x / deeper.w).abs() < 1e-5);
                assert!(camera.view.is_finite() && camera.projection.is_finite());
                let near =
                    matrix * (position + camera.forward * CameraSpec::ORTHOGRAPHIC_NEAR).extend(1.);
                let far = matrix * (position + camera.forward * 1_000_000.).extend(1.);
                assert!(near.z > far.z, "depth must remain reversed-Z");
            }
        }
        assert!(
            CameraSpec::OrthographicLookAt {
                position: Vec3::ZERO,
                target: Vec3::ZERO,
                vertical_size: 4.
            }
            .resolve([800, 600])
            .is_err()
        );
        assert!(
            CameraSpec::OrthographicLookAt {
                position: Vec3::Z,
                target: Vec3::ZERO,
                vertical_size: f32::NAN
            }
            .resolve([800, 600])
            .is_err()
        );
    }
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
