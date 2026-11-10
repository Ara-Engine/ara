//! Keeps Renderlab's scene equal to the extracted frame's draws. Geometry and
//! materials are shared by content key, objects follow their entity, and skinned
//! primitives are posed on the GPU from the frame's palettes. Renderlab draws at
//! most `TRACED_DRAWS` mesh-material groups per frame; draws beyond that are left
//! out and reported (`ARA_RENDERLAB_LIMIT`).
use crate::{gaps::Gaps, texture};
use anyhow::{Context, Result};
use ara_runtime::{
    extract::{Draw, Geometry, RenderFrame},
    model::{ModelMaterial, ModelTexture, ModelVertex, TextureUsage},
};
use renderlab::{
    asset::{TangentBasis, TextureRole},
    engine::{
        Engine, Influence, Mat4, MaterialDesc, MaterialId, MeshData, MeshId, ObjectId, Primitive,
        ProgramId, Sampler, TangentStream, TextureId, TextureRef, Transparency, Value, Vertex,
    },
};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

/// Mesh-material groups Renderlab draws in one frame.
const MAX_GROUPS: usize = renderlab::TRACED_DRAWS as usize;
const FNV: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A resource shared by the objects that use it; released with its last user.
struct Shared<T> {
    id: T,
    users: u32,
}

/// Geometry an object draws: shared static triangles, or its own posed mesh.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum MeshKey {
    Static(u64),
    Skinned(u64),
}

struct Object {
    id: ObjectId,
    mesh: MeshKey,
    material: u64,
    /// The shared material in use: `material` and whether the mesh has tangents.
    material_slot: u64,
    world: [f32; 16],
    /// The posed mesh and joint count of a skinned object.
    skin: Option<(MeshId, usize)>,
}

/// Renderlab handles for everything the frame draws.
#[derive(Default)]
pub(crate) struct Scene {
    /// Static meshes by geometry key, and whether each was uploaded with tangents.
    meshes: HashMap<u64, (Shared<MeshId>, bool)>,
    materials: HashMap<u64, Shared<MaterialId>>,
    textures: HashMap<(u64, u8), TextureId>,
    objects: HashMap<u64, Object>,
    palette: Vec<Mat4>,
    /// The emissive program once tried: `Some(None)` when Renderlab rejected it.
    emissive: Option<Option<ProgramId>>,
}

/// Renderlab's standard program with a `glow` parameter (cd/m²) that the surface
/// both shows and, through the `emission` hook, sends into global illumination.
/// None when the standard program no longer ends the way this edit expects.
fn emissive_source() -> Option<String> {
    const RETURN: &str = "    return exposed;";
    const GLOW: &str = "max(params.glow,vec3<f32>(0.0))";
    let standard = renderlab::STANDARD_PROGRAM;
    (standard.matches(RETURN).count() == 1 && !standard.contains("struct Parameters")).then(|| {
        format!(
            "struct Parameters {{ glow:vec3<f32>, unused:f32 }};\n\
             const DEFAULT_PARAMETERS:Parameters = Parameters(vec3<f32>(0.0),0.0);\n\
             {}\n\
             fn emission(s:Surface, input:SurfaceInput) -> vec3<f32> {{ return {GLOW}; }}\n",
            standard.replace(
                RETURN,
                &format!(
                    "    return min(exposed+exposed_product({GLOW},vec3<f32>(1.0)),vec3<f32>(65504.0));"
                ),
            )
        )
    })
}

/// The material a draw resolves to: the native asset's or the model slot's, with
/// the textures its indices address.
fn source(draw: &Draw) -> Option<(&ModelMaterial, &[ModelTexture])> {
    if let Some(native) = draw.material.as_deref() {
        return Some((&native.material, &native.textures));
    }
    let Geometry::Model { model, primitive } = &draw.geometry else {
        return None;
    };
    let slot = model.primitives.get(*primitive)?.material?;
    Some((model.materials.get(slot)?, &model.textures))
}

/// Whether the draw's material samples a normal map (its mesh then needs tangents).
fn normal_mapped(draw: &Draw) -> bool {
    source(draw).is_some_and(|(m, _)| m.normal_texture.is_some())
}

fn geometry_key(draw: &Draw) -> u64 {
    let hash = match &draw.geometry {
        Geometry::Shape { shape, size } => size.iter().fold(fnv(FNV, &[*shape as u8]), |h, v| {
            fnv(h, &v.to_bits().to_le_bytes())
        }),
        Geometry::Model { model, primitive } => fnv(
            fnv(FNV, &model.digest.0),
            &(*primitive as u64).to_le_bytes(),
        ),
    };
    fnv(hash, &[u8::from(normal_mapped(draw))])
}

fn material_key(draw: &Draw) -> u64 {
    let mut hash = FNV;
    for value in draw
        .color
        .iter()
        .chain([draw.roughness, draw.metallic].iter())
        .chain(draw.emissive.iter())
        .chain(draw.emissive_factor.iter().flatten())
    {
        hash = fnv(hash, &value.to_bits().to_le_bytes());
    }
    hash = fnv(hash, &[u8::from(draw.emissive_factor.is_some())]);
    if let Some(native) = &draw.material {
        hash = fnv(fnv(hash, b"native"), &native.digest.0);
    } else if let Geometry::Model { model, primitive } = &draw.geometry {
        let slot = model
            .primitives
            .get(*primitive)
            .and_then(|p| p.material)
            .map_or(u64::MAX, |slot| slot as u64);
        hash = fnv(fnv(hash, &model.digest.0), &slot.to_le_bytes());
    }
    hash
}

fn vertex(v: &ModelVertex) -> Vertex {
    let normal = glam_normalize(v.normal);
    Vertex {
        position: v.position,
        normal,
        uv: v.uv,
        color: v.color.map(|c| f32::from(c) / 255.),
    }
}

fn glam_normalize(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length.is_finite() && length > 1e-12 {
        v.map(|c| c / length)
    } else {
        [0., 1., 0.]
    }
}

/// Unit tangents orthogonal to their normals with a ±1 handedness, as Renderlab's
/// tangent validation expects; None when any tangent is unusable.
fn tangents(vertices: &[ModelVertex]) -> Option<TangentStream> {
    let mut values = Vec::with_capacity(vertices.len());
    for v in vertices {
        let n = glam_normalize(v.normal);
        let t = [v.tangent[0], v.tangent[1], v.tangent[2]];
        let along = t[0] * n[0] + t[1] * n[1] + t[2] * n[2];
        let t = [
            t[0] - n[0] * along,
            t[1] - n[1] * along,
            t[2] - n[2] * along,
        ];
        let length = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
        if !length.is_finite() || length < 1e-6 {
            return None;
        }
        let sign = if v.tangent[3] < 0. { -1. } else { 1. };
        values.push([t[0] / length, t[1] / length, t[2] / length, sign]);
    }
    Some(TangentStream {
        basis: TangentBasis::Authored,
        values,
    })
}

/// Renderlab rejects a triangle whose corners disagree on tangent handedness (a
/// mirrored-UV seam shared by welded vertices). Gives each odd corner its own copy
/// of the vertex with the triangle's majority handedness.
fn split_handedness(vertices: &mut Vec<ModelVertex>, indices: &mut [u32]) {
    if indices.iter().any(|i| *i as usize >= vertices.len()) {
        return;
    }
    let mut copies: HashMap<u32, u32> = HashMap::new();
    for triangle in indices.as_chunks_mut::<3>().0 {
        let negative = triangle
            .iter()
            .filter(|i| vertices[**i as usize].tangent[3] < 0.)
            .count();
        if negative == 0 || negative == 3 {
            continue;
        }
        let majority_negative = negative >= 2;
        for index in triangle.iter_mut() {
            let vertex = vertices[*index as usize];
            if (vertex.tangent[3] < 0.) != majority_negative {
                *index = *copies.entry(*index).or_insert_with(|| {
                    let mut copy = vertex;
                    copy.tangent[3] = if majority_negative { -1. } else { 1. };
                    vertices.push(copy);
                    (vertices.len() - 1) as u32
                });
            }
        }
    }
}
/// A vertex's four strongest influences, renormalized. Rigid vertices of a skinned
/// primitive (no weights) and out-of-range joints bind to the identity joint
/// appended at `joints`, as Ara's CPU skinning leaves them unposed.
fn influence(v: &ModelVertex, joints: usize) -> Influence {
    let mut ranked: Vec<(u16, f32)> = v
        .joints
        .iter()
        .zip(&v.weights)
        .filter(|(joint, weight)| **weight > 0. && (**joint as usize) < joints)
        .map(|(joint, weight)| (*joint, *weight))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.truncate(4);
    let total: f32 = ranked.iter().map(|(_, w)| w).sum();
    let mut out = Influence {
        joints: [joints as u16; 4],
        weights: [0.; 4],
    };
    if total <= 0. {
        out.weights[0] = 1.;
        return out;
    }
    for (slot, (joint, weight)) in ranked.into_iter().enumerate() {
        out.joints[slot] = joint;
        out.weights[slot] = weight / total;
    }
    // Exactly one: the sum of rounded quotients may miss it by a few ulps.
    let rest: f32 = out.weights[1..].iter().sum();
    out.weights[0] = 1. - rest;
    out
}

/// The draw's model-space vertices and indices.
type Triangles<'a> = (Cow<'a, [ModelVertex]>, Cow<'a, [u32]>);

/// The draw's model-space vertices and indices.
fn triangles(draw: &Draw) -> Result<Triangles<'_>> {
    Ok(match &draw.geometry {
        Geometry::Shape { shape, size } => {
            let source = ara_runtime_render::geometry::shape(*shape, *size);
            (source.vertices.into(), source.indices.into())
        }
        Geometry::Model { model, primitive } => {
            let p = model
                .primitives
                .get(*primitive)
                .context("primitive out of range")?;
            (p.vertices.as_slice().into(), p.indices.as_slice().into())
        }
    })
}

impl Scene {
    /// Makes Renderlab's objects equal `frame.draws`.
    pub fn sync(
        &mut self,
        engine: &mut Engine,
        frame: &RenderFrame,
        gaps: &mut Gaps,
    ) -> Result<()> {
        let mut live = HashSet::with_capacity(frame.draws.len());
        let mut groups = HashSet::new();
        let mut dropped = 0usize;
        for draw in &frame.draws {
            let mesh = match draw.skin {
                Some(_) => MeshKey::Skinned(draw.entity),
                None => MeshKey::Static(geometry_key(draw)),
            };
            let material = material_key(draw);
            if !groups.contains(&(mesh, material)) {
                if groups.len() == MAX_GROUPS {
                    dropped += 1;
                    continue;
                }
                groups.insert((mesh, material));
            }
            if draw.paint.is_some() {
                gaps.unsupported(
                    "per-instance vertex paint",
                    "the mesh keeps its own colours",
                );
            }
            live.insert(draw.entity);
            self.place(engine, frame, draw, mesh, material, gaps)?;
        }
        if dropped > 0 {
            gaps.limit(
                "64 mesh-material groups per frame",
                &format!("{dropped} draws of this frame are not drawn"),
            );
        }
        let gone: Vec<u64> = self
            .objects
            .keys()
            .filter(|entity| !live.contains(entity))
            .copied()
            .collect();
        for entity in gone {
            self.remove(engine, entity)?;
        }
        Ok(())
    }

    /// Spawns, replaces or moves the object of one draw.
    fn place(
        &mut self,
        engine: &mut Engine,
        frame: &RenderFrame,
        draw: &Draw,
        mesh: MeshKey,
        material: u64,
        gaps: &mut Gaps,
    ) -> Result<()> {
        let world = draw.world.to_cols_array();
        let joints = draw.skin.as_ref().map(|range| range.len());
        let reusable = self.objects.get(&draw.entity).is_some_and(|o| {
            o.mesh == mesh && o.material == material && o.skin.map(|s| s.1) == joints
        });
        if !reusable {
            self.remove(engine, draw.entity)?;

            let (mesh_id, skin, tangents) = match (mesh, joints) {
                (MeshKey::Static(key), _) => {
                    let (id, tangents) = self.mesh(engine, draw, key, gaps)?;
                    (id, None, tangents)
                }
                (MeshKey::Skinned(_), joints) => {
                    let joints = joints.unwrap_or(0);
                    let (id, tangents) = skinned_mesh(engine, draw, joints)?;
                    (id, Some((id, joints)), tangents)
                }
            };
            // A material samples its normal map only on a mesh that has tangents.
            let material_slot = fnv(material, &[u8::from(tangents)]);
            let material_id = self.material(engine, draw, material_slot, tangents, gaps)?;
            let id = engine.spawn(mesh_id, material_id, Mat4::from_cols_array(&world))?;
            self.objects.insert(
                draw.entity,
                Object {
                    id,
                    mesh,
                    material,
                    material_slot,
                    world,
                    skin,
                },
            );
        }
        let object = self
            .objects
            .get_mut(&draw.entity)
            .context("object placed above")?;
        if object.world != world {
            engine.set_transform(object.id, Mat4::from_cols_array(&world))?;
            object.world = world;
        }
        if let (Some((posed, joints)), Some(range)) = (object.skin, draw.skin.clone()) {
            self.palette.clear();
            self.palette.extend(
                frame.palettes[range]
                    .iter()
                    .map(|m| Mat4::from_cols_array(&m.to_cols_array())),
            );
            debug_assert_eq!(self.palette.len(), joints);
            // The identity joint rigid vertices bind to (`influence`).
            self.palette.push(Mat4::IDENTITY);
            // Sent every frame: Renderlab itself ignores an unchanged palette.
            engine.set_joint_palette(posed, &self.palette)?;
        }
        Ok(())
    }

    /// Despawns `entity`'s object and releases what only it used.
    fn remove(&mut self, engine: &mut Engine, entity: u64) -> Result<()> {
        let Some(object) = self.objects.remove(&entity) else {
            return Ok(());
        };
        engine.despawn(object.id)?;
        match (object.mesh, object.skin) {
            (_, Some((posed, _))) => engine.remove_mesh(posed)?,
            (MeshKey::Static(key), None) => {
                if let Some((shared, _)) = self.meshes.get_mut(&key) {
                    shared.users -= 1;
                    if shared.users == 0 {
                        engine.remove_mesh(shared.id)?;
                        self.meshes.remove(&key);
                    }
                }
            }
            (MeshKey::Skinned(_), None) => {}
        }
        if let Some(shared) = self.materials.get_mut(&object.material_slot) {
            shared.users -= 1;
            if shared.users == 0 {
                engine.remove_material(shared.id)?;
                self.materials.remove(&object.material_slot);
            }
        }
        Ok(())
    }

    /// The shared static mesh for `key`, uploaded on first use, and whether it
    /// carries tangents.
    fn mesh(
        &mut self,
        engine: &mut Engine,
        draw: &Draw,
        key: u64,
        gaps: &mut Gaps,
    ) -> Result<(MeshId, bool)> {
        if let Some((shared, tangents)) = self.meshes.get_mut(&key) {
            shared.users += 1;
            return Ok((shared.id, *tangents));
        }
        let wanted = normal_mapped(draw);
        let (mut source, mut indices) = triangles(draw)?;
        if wanted {
            split_handedness(source.to_mut(), indices.to_mut());
        }
        let vertices: Vec<Vertex> = source.iter().map(vertex).collect();
        let data = MeshData {
            vertices: &vertices,
            indices: &indices,
        };
        let frames = wanted.then(|| tangents(&source)).flatten();
        let uploaded = frames
            .as_ref()
            .map(|frames| engine.create_mesh_with_tangents(&data, frames));
        let (id, tangents) = match uploaded {
            Some(Ok(id)) => (id, true),
            rejected => {
                if wanted {
                    let why = match rejected {
                        Some(Err(error)) => error.to_string(),
                        _ => "a tangent is missing or lies along its normal".to_owned(),
                    };
                    gaps.unsupported(
                        "normal maps on meshes without usable tangents",
                        &format!("`{}` shades with its vertex normals ({why})", draw.label),
                    );
                }
                (engine.create_mesh(&data)?, false)
            }
        };
        self.meshes.insert(key, (Shared { id, users: 1 }, tangents));
        Ok((id, tangents))
    }

    /// The shared material for `key`, created on first use; it samples its normal
    /// map only when its mesh has `tangents`.
    fn material(
        &mut self,
        engine: &mut Engine,
        draw: &Draw,
        key: u64,
        tangents: bool,
        gaps: &mut Gaps,
    ) -> Result<MaterialId> {
        if let Some(shared) = self.materials.get_mut(&key) {
            shared.users += 1;
            return Ok(shared.id);
        }
        let parameters;
        let mut desc = MaterialDesc {
            base_color: draw.color,
            metallic: draw.metallic,
            roughness: draw.roughness,
            // Renderlab's shadowed paths need double-sided materials today.
            double_sided: true,
            ..Default::default()
        };
        // Emitted radiance in cd/m². Shapes carry it directly; a model without a
        // material emits only through an explicit factor (as `ara-runtime-render`
        // lowers them).
        let scaled = |factor: [f32; 3]| std::array::from_fn(|i| factor[i] * draw.emissive[i]);
        let mut glow: [f32; 3] = match &draw.geometry {
            Geometry::Shape { .. } => draw.emissive,
            Geometry::Model { .. } => scaled(draw.emissive_factor.unwrap_or([0.; 3])),
        };
        let mut emissive_map = false;
        // Shapes blend through their colour alpha.
        let mut blend = draw.color[3] < 1.;
        if let Some((m, textures)) = source(draw) {
            for (out, (a, b)) in desc
                .base_color
                .iter_mut()
                .zip(m.base_color.iter().zip(draw.color))
            {
                *out = a * b;
            }
            desc.metallic = m.metallic;
            desc.roughness = m.roughness;
            desc.normal_scale = m.normal_scale;
            let factor = draw.emissive_factor.unwrap_or(m.emissive);
            glow = scaled(factor);
            emissive_map = m.emissive_texture.is_some();
            let mut bind = |slot: Option<usize>, role: TextureRole| -> Result<Option<TextureRef>> {
                let Some(texture) = slot.and_then(|index| textures.get(index)) else {
                    return Ok(None);
                };
                Ok(Some(TextureRef {
                    texture: self.texture(engine, texture, role)?,
                    sampler: Sampler::default(),
                }))
            };
            desc.base_color_texture = bind(m.base_color_texture, TextureRole::BaseColorSrgb)?;
            desc.metallic_roughness_texture = bind(
                m.metallic_roughness_texture,
                TextureRole::MetallicRoughnessLinear,
            )?;
            if tangents {
                desc.normal_texture = bind(m.normal_texture, TextureRole::NormalLinear)?;
            }
            if m.occlusion_texture.is_some() {
                gaps.unsupported(
                    "ambient-occlusion maps",
                    "occlusion comes from traced GI only",
                );
            }
            // A uniform alpha below one is see-through; alpha that lives only in the
            // base-colour map (masks, opacity maps) needs cutouts Renderlab makes from
            // geometry, so those draw opaque.
            let mapped = m.alpha_mode != 0 && m.base_color_texture.is_some();
            blend = m.alpha_mode == 2 && desc.base_color[3] < 1.;
            if mapped && !blend {
                gaps.unsupported(
                    "alpha masks and opacity maps",
                    "those materials draw opaque, without cutouts",
                );
                desc.base_color[3] = 1.;
            }
            // glTF transmission belongs to the dielectric part only: a metal does not
            // transmit. Metalness or transmission that varies over the surface (a map)
            // is no uniform glass, so those stay opaque.
            let transmission = m.transmission_factor * (1. - m.metallic).clamp(0., 1.);
            let varying = m.transmission_texture.is_some()
                || (m.metallic > 0. && m.metallic_roughness_texture.is_some());
            if m.transmission_factor > 0. && varying {
                gaps.unsupported(
                    "transmission that varies over a surface",
                    "those materials draw opaque (uniform transmission is glass)",
                );
            } else if transmission > 0. {
                // Tinted glass colours light by absorption through its volume;
                // Renderlab's thin slab tints by base colour, so fold the absorption
                // over the slab's thickness into it (Beer-Lambert).
                let depth = m.volume.thickness / m.volume.attenuation_distance;
                if depth.is_finite() && depth > 0. {
                    for (base, tint) in desc.base_color.iter_mut().zip(m.volume.attenuation_color) {
                        *base *= tint.clamp(0., 1.).powf(depth);
                    }
                }
                desc.transparency = Some(Transparency {
                    opacity: (1. - transmission).clamp(0., 1.),
                    ior: m.volume.ior.clamp(1., 3.),
                    thickness: m.volume.thickness.max(0.),
                });
            }
            if draw.material.is_some() {
                gaps.unsupported(
                    "material-graph materials",
                    "they shade with their base factors and maps on Renderlab's standard program",
                );
            }
        }
        let glow = glow.map(|c| if c.is_finite() { c.max(0.) } else { 0. });
        if glow.iter().any(|c| *c > 0.) {
            if emissive_map {
                gaps.unsupported("emissive maps", "the material does not emit or glow");
            } else if let Some(program) = self.emissive_program(engine, gaps) {
                parameters = [("glow", Value::Vec3(glow))];
                desc.program = Some(program);
                desc.parameters = &parameters;
            }
        }
        if blend && desc.transparency.is_none() {
            desc.transparency = Some(Transparency {
                opacity: 1.,
                ior: 1.,
                thickness: 0.,
            });
        }
        let id = engine.create_material(&desc)?;
        self.materials.insert(key, Shared { id, users: 1 });
        Ok(id)
    }

    /// The program emissive materials shade with, composed on first use; None (and
    /// one report) when Renderlab rejects it.
    fn emissive_program(&mut self, engine: &mut Engine, gaps: &mut Gaps) -> Option<ProgramId> {
        if self.emissive.is_none() {
            let created = emissive_source()
                .ok_or_else(|| "its standard program changed shape".to_owned())
                .and_then(|source| {
                    engine
                        .create_program("ara-standard-emissive", &source)
                        .map_err(|error| error.to_string())
                });
            if let Err(error) = &created {
                gaps.unsupported(
                    "emissive materials",
                    &format!("they do not emit or glow ({error})"),
                );
            }
            self.emissive = Some(created.ok());
        }
        self.emissive.flatten()
    }

    /// The Renderlab texture of one cooked Ara texture in `role`, uploaded once.
    /// Keyed by the cooked bytes, so every model sharing a texture shares its upload.
    fn texture(
        &mut self,
        engine: &mut Engine,
        texture: &ModelTexture,
        role: TextureRole,
    ) -> Result<TextureId> {
        let role_tag = match (role, texture.usage) {
            (TextureRole::BaseColorSrgb, _) => 0,
            (TextureRole::NormalLinear, TextureUsage::Normal) => 2,
            _ => 1,
        };
        let key = (fnv(FNV, &texture.ktx2), role_tag);
        if let Some(id) = self.textures.get(&key) {
            return Ok(*id);
        }
        let id = engine.create_texture(texture::prepare(&texture.ktx2, role)?)?;
        self.textures.insert(key, id);
        Ok(id)
    }
}

/// Uploads one skinned primitive with its own pose (`joints` plus the identity joint).
/// Returns the mesh and whether it carries tangents.
fn skinned_mesh(engine: &mut Engine, draw: &Draw, joints: usize) -> Result<(MeshId, bool)> {
    let (mut source, mut indices) = triangles(draw)?;
    if normal_mapped(draw) {
        split_handedness(source.to_mut(), indices.to_mut());
    }
    let primitive = Primitive {
        vertices: source.iter().map(vertex).collect(),
        tangents: normal_mapped(draw).then(|| tangents(&source)).flatten(),
        indices: indices.to_vec(),
        material: 0,
    };
    let influences: Vec<Influence> = source.iter().map(|v| influence(v, joints)).collect();
    let mesh = engine.create_skinned_mesh(&primitive, &influences, joints as u32 + 1)?;
    Ok((mesh, primitive.tangents.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn influences_keep_the_four_strongest_and_rigid_vertices_bind_identity() {
        let mut v = ModelVertex {
            joints: [0, 1, 2, 3, 4, 9, 0, 0],
            weights: [0.05, 0.3, 0.2, 0.1, 0.25, 0.1, 0., 0.],
            ..Default::default()
        };
        // Joint 9 is outside a six-joint skin and is ignored.
        let i = influence(&v, 6);
        assert_eq!(i.joints, [1, 4, 2, 3]);
        assert!((i.weights.iter().sum::<f32>() - 1.).abs() < 1e-6);
        assert!((i.weights[0] - 0.3 / 0.85).abs() < 1e-5);
        v.weights = [0.; 8];
        let rigid = influence(&v, 6);
        assert_eq!((rigid.joints[0], rigid.weights), (6, [1., 0., 0., 0.]));
    }

    #[test]
    fn emissive_program_extends_the_pinned_standard_program() {
        let source = emissive_source().expect("the standard program ends as expected");
        assert!(source.contains("fn emission(") && source.contains("params.glow"));
        assert!(!source.contains("    return exposed;"));
    }

    #[test]
    fn handedness_seams_split_the_odd_corner() {
        let corner = |sign: f32| ModelVertex {
            tangent: [1., 0., 0., sign],
            ..Default::default()
        };
        // Two triangles share vertices 1 and 2; vertex 2 is mirrored.
        let mut vertices = vec![corner(1.), corner(1.), corner(-1.), corner(-1.)];
        let mut indices = [0, 1, 2, 2, 1, 3];
        split_handedness(&mut vertices, &mut indices);
        // Each triangle now agrees with its majority; the copies are appended.
        assert_eq!(indices, [0, 1, 4, 2, 5, 3]);
        assert_eq!((vertices[4].tangent[3], vertices[5].tangent[3]), (1., -1.));
        for triangle in indices.as_chunks::<3>().0 {
            let signs: Vec<bool> = triangle
                .iter()
                .map(|i| vertices[*i as usize].tangent[3] < 0.)
                .collect();
            assert!(signs.iter().all(|s| *s == signs[0]));
        }
    }

    #[test]
    fn tangents_are_orthonormal_or_absent() {
        let mut v = ModelVertex {
            normal: [0., 1., 0.],
            tangent: [1., 1., 0., -1.],
            ..Default::default()
        };
        let stream = tangents(&[v]).expect("usable tangent");
        assert_eq!(stream.values[0], [1., 0., 0., -1.]);
        v.tangent = [0., 2., 0., 1.];
        assert!(
            tangents(&[v]).is_none(),
            "a tangent along the normal is unusable"
        );
    }
}
