//! Keeps Renderlab's scene equal to the extracted frame's draws. Geometry and
//! materials are shared by content key, objects follow their entity, and skinned
//! primitives are posed on the GPU from the frame's palettes. Renderlab draws at
//! most `TRACED_DRAWS` mesh-material groups per frame; past that, static groups merge
//! into world-space batches per material (`batch`), and only what still does not fit
//! is left out. Both are reported (`ARA_RENDERLAB_LIMIT`). Draws the bridge derives
//! itself (foliage) are placed once per content key and then left alone.
use crate::rl::Engine;
use crate::{
    batch::{self, Fate, Group},
    cutout,
    gaps::Gaps,
    texture,
};
use anyhow::{Context, Result};
use ara_runtime::{
    extract::{Draw, Geometry, RenderFrame},
    model::{ModelMaterial, ModelTexture, ModelVertex, TextureUsage},
};
use renderlab::{
    asset::{TangentBasis, TextureRole},
    engine::{
        Influence, Mat4, MaterialDesc, MaterialId, MeshData, MeshId, ObjectId, Primitive,
        ProgramId, Sampler, TangentStream, TextureId, TextureRef, Transparency, Value, Vertex,
    },
};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, hash_map::Entry},
};

/// Mesh-material groups Renderlab draws in one frame.
pub(crate) const MAX_GROUPS: usize = renderlab::TRACED_DRAWS as usize;
const FNV: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Everything of a frame's own draws (not the bridge's foliage) that [`Scene::sync`] reads, as
/// one 64-bit word each folded in: two frames with the same signature make the same objects.
fn draws_signature(frame: &RenderFrame) -> u64 {
    let mut h = 0x9e37_79b9_7f4a_7c15_u64;
    let mut word = |w: u64| h = (h ^ w).wrapping_mul(0xff51_afd7_ed55_8ccd).rotate_left(31);
    let float = |v: f32| u64::from(v.to_bits());
    word(frame.draws.len() as u64);
    for draw in &frame.draws {
        word(draw.entity);
        for v in draw.world.to_cols_array() {
            word(float(v));
        }
        for v in draw
            .color
            .into_iter()
            .chain([draw.roughness, draw.metallic])
        {
            word(float(v));
        }
        for v in draw
            .emissive
            .into_iter()
            .chain(draw.emissive_factor.into_iter().flatten())
        {
            word(float(v));
        }
        word(u64::from(draw.emissive_factor.is_some()));
        word(u64::from(draw.casts_shadows) | u64::from(draw.receives_decals) << 1);
        match &draw.geometry {
            Geometry::Shape { shape, size } => {
                word(*shape as u64);
                size.iter().for_each(|v| word(float(*v)));
            }
            Geometry::Model { model, primitive } => {
                model.digest.0.chunks(8).for_each(|c| {
                    word(u64::from_le_bytes(std::array::from_fn(|i| {
                        *c.get(i).unwrap_or(&0)
                    })));
                });
                word(*primitive as u64);
            }
        }
        if let Some(native) = &draw.material {
            native.digest.0.chunks(8).for_each(|c| {
                word(u64::from_le_bytes(std::array::from_fn(|i| {
                    *c.get(i).unwrap_or(&0)
                })));
            });
        }
        word(
            draw.paint
                .as_ref()
                .map_or(0, |p| p.as_ptr() as u64 ^ p.len() as u64),
        );
        if let Some(range) = &draw.skin {
            word(range.start as u64 ^ (range.len() as u64) << 32);
            for joint in &frame.palettes[range.clone()] {
                for v in joint.to_cols_array() {
                    word(float(v));
                }
            }
        }
    }
    h
}

/// What the last [`Scene::sync`] saw, so an unchanged frame can skip the whole pass.
#[derive(PartialEq, Eq, Clone, Copy)]
struct Synced {
    draws: u64,
    static_key: u64,
    reserved: usize,
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

/// A draw's transform with its last row made exactly (0, 0, 0, 1) when it is within 1e-3 of it:
/// transforms composed in `f32` (the editor's extraction) end in 0.99999994, which Renderlab
/// rejects as not affine. A matrix further from affine is left as it is.
fn cleaned(world: &glam34::Mat4) -> glam34::Mat4 {
    let mut m = *world;
    if m.x_axis.w.abs() < 1e-3
        && m.y_axis.w.abs() < 1e-3
        && m.z_axis.w.abs() < 1e-3
        && (m.w_axis.w - 1.).abs() < 1e-3
    {
        (m.x_axis.w, m.y_axis.w, m.z_axis.w, m.w_axis.w) = (0., 0., 0., 1.);
    }
    m
}

/// Whether a draw's transform is one Renderlab can light (`NormalTransform::valid`): an exactly
/// affine matrix whose 3x3 part is invertible with max|A| * max|A^-1| within 1e4, so its normals
/// are defined. Renderlab refuses the whole frame for a draw that is not, and Ara's renderer
/// culls such draws (an entity at zero scale, a hidden one), so the bridge leaves them out.
fn drawable(world: &glam34::Mat4) -> bool {
    if !world.is_finite() {
        return false;
    }
    let m = cleaned(world).to_cols_array_2d();
    if m[0][3] != 0. || m[1][3] != 0. || m[2][3] != 0. || m[3][3] != 1. {
        return false;
    }
    let a = glam34::DMat3::from_cols(
        glam34::DVec3::new(m[0][0].into(), m[0][1].into(), m[0][2].into()),
        glam34::DVec3::new(m[1][0].into(), m[1][1].into(), m[1][2].into()),
        glam34::DVec3::new(m[2][0].into(), m[2][1].into(), m[2][2].into()),
    );
    let det = a.determinant();
    if det == 0. || !det.is_finite() {
        return false;
    }
    let inverse = a.inverse();
    let max = |a: glam34::DMat3| {
        a.to_cols_array()
            .into_iter()
            .map(f64::abs)
            .fold(0., f64::max)
    };
    inverse.is_finite() && max(a) * max(inverse) <= 1e4
}

/// Object ids by owner: own objects by entity (`false`), batches by material key (`true`),
/// each with the object and its `(first primitive, count, id)` ranges.
type Tags = HashMap<(bool, u64), (ObjectId, Vec<(u32, u32, u32)>)>;

/// One material's static batch: its members' triangles merged in world space.
struct Batch {
    /// The members' geometry and transforms the mesh was built from.
    content: u64,
    id: ObjectId,
    mesh: MeshId,
    material_slot: u64,
    /// Each member in mesh order: its draw's entity and its triangle count, so the batch's
    /// object ids can name each member's primitives (`Scene::tag`).
    layout: Vec<(u64, u32)>,
}

/// Renderlab handles for everything the frame draws.
#[derive(Default)]
pub(crate) struct Scene {
    /// Static meshes by geometry key, and whether each was uploaded with tangents.
    meshes: HashMap<u64, (Shared<MeshId>, bool)>,
    materials: HashMap<u64, Shared<MaterialId>>,
    textures: HashMap<(u64, u8), TextureId>,
    objects: HashMap<u64, Object>,
    /// Static batches by material key (`batch`).
    batches: HashMap<u64, Batch>,
    /// Key of the static draws last placed (`sync`'s `static_key`).
    static_key: Option<u64>,
    /// The inputs of the last completed `sync`.
    synced: Option<Synced>,
    /// The ids last set on Renderlab's objects, with the object they were set on: own
    /// objects by entity (`false`), batches by material key (`true`).
    tags: Tags,
    palette: Vec<Mat4>,
    /// The emissive program once tried: `Some(None)` when Renderlab rejected it.
    emissive: Option<Option<ProgramId>>,
    /// Mean linear colour of each emissive map packed so far, by its content key.
    glow_means: HashMap<u64, [f32; 3]>,
    /// Triangles of the largest mesh uploaded so far (`largest_triangles`).
    largest: usize,
    /// What each blended material's base-colour map does with its alpha.
    coverage: Coverage,
}

/// What a draw's blended material shows through, and where: the alpha class
/// (`cutout::Alpha`) of base-colour maps already looked at, by the identity of their
/// KTX2 bytes, and which primitives fade by vertex colour. Each map entry keeps a weak
/// handle on its bytes, so an address is never reused while its entry lives; `prune`
/// drops entries of freed maps.
#[derive(Default)]
struct Coverage {
    known: HashMap<(usize, usize), Seen>,
    /// Whether each model primitive's vertex colour alpha fades it (`FADED_BELOW`),
    /// by model digest and primitive. A few bytes per primitive, never pruned.
    faded: HashMap<(u64, usize), bool>,
}

/// One map `Coverage` has looked at: its bytes, held weakly, and what its alpha does.
struct Seen {
    bytes: std::sync::Weak<[u8]>,
    alpha: MapAlpha,
}

/// What a base-colour map's alpha does (`cutout::alpha`), and whether it could be read.
#[derive(Clone, Copy)]
struct MapAlpha {
    class: cutout::Alpha,
    /// The share of visible texels that are partly there.
    partly: f32,
    /// False when the map did not decode; it then counts as opaque.
    decoded: bool,
}

/// A vertex colour alpha under this (of 255) fades a primitive: Renderlab's transparent
/// pass multiplies coverage by it, as it does the alpha factor and the map's alpha.
const FADED_BELOW: u8 = 250;

/// A there-or-not map with more than this share of its visible texels partly there is
/// reported: those texels are cut or kept at half coverage.
const PARTLY_REPORTED: f32 = 0.03;

impl Coverage {
    /// What `texture`'s alpha does, decoded on first sight. A map that does not decode
    /// counts as opaque (`decoded` is false so the caller can say so).
    fn of(&mut self, texture: &ModelTexture) -> MapAlpha {
        let key = (texture.ktx2.as_ptr() as usize, texture.ktx2.len());
        self.known
            .entry(key)
            .or_insert_with(|| {
                let alpha = match ara_runtime_render::decode_texture(&texture.ktx2) {
                    Ok(decoded) => {
                        let stats = cutout::alpha(&decoded.rgba);
                        MapAlpha {
                            class: stats.class,
                            partly: stats.partly,
                            decoded: true,
                        }
                    }
                    Err(_) => MapAlpha {
                        class: cutout::Alpha::Opaque,
                        partly: 0.,
                        decoded: false,
                    },
                };
                Seen {
                    bytes: std::sync::Arc::downgrade(&texture.ktx2),
                    alpha,
                }
            })
            .alpha
    }

    /// Whether the draw's primitive fades by vertex colour alpha. Shapes carry none.
    fn faded(&mut self, draw: &Draw) -> bool {
        let Geometry::Model { model, primitive } = &draw.geometry else {
            return false;
        };
        let key = (fnv(FNV, &model.digest.0), *primitive);
        *self.faded.entry(key).or_insert_with(|| {
            model
                .primitives
                .get(*primitive)
                .is_some_and(|p| p.vertices.iter().any(|v| v.color[3] < FADED_BELOW))
        })
    }

    fn prune(&mut self) {
        self.known.retain(|_, seen| seen.bytes.strong_count() > 0);
    }
}

/// Where a material's textures carry its emissive map (`texture::with_glow`): red in
/// the metallic-roughness texture's red, green in its alpha, blue in the base colour's
/// alpha. As (host channel, emissive channel) per texture.
const GLOW_IN_DATA: [(usize, usize); 2] = [(0, 0), (3, 1)];
const GLOW_IN_BASE: [(usize, usize); 1] = [(3, 2)];
/// The base-colour alpha of an opaque emissive material, which Renderlab's opaque
/// shading never reads. Renderlab shades a reflection ray's hit with the reflecting
/// surface's program and the hit's material, so an emissive surface's reflections
/// would show every object glowing with its `glow`; the emissive program glows only on
/// a material that carries this mark.
const GLOW_MARK: f32 = 0.737;

/// Renderlab's standard program with a `glow` parameter (cd/m²) that the surface
/// both shows and, through the `emission` hook, sends into global illumination. Where
/// the `mapped` parameter is one, the glow is multiplied by the emissive map packed
/// into the material's textures; where it is zero the glow is uniform. One program
/// serves both: every program is another set of pipelines to compile on a cold start
/// and another pass in Renderlab's shading. The map is read in `surface`, for the
/// pixels the camera sees. Global illumination calls `emission` at its own hits, where
/// a texture lookup does not return the material's texels, so it gets `bounce`, the
/// glow averaged over the map, as a second parameter. None when the standard program
/// no longer has the shape these edits expect.
fn emissive_source() -> Option<String> {
    const FIELDS: &str = "alpha:f32 };";
    const BUILT: &str = "decal_mr.x,alpha);";
    const RETURN: &str = "    return exposed;";
    const MAP: &str = "vec3<f32>(mr.r,mr.a,sampled.a)*vec3<f32>(mr.r,mr.a,sampled.a)";
    let standard = renderlab::STANDARD_PROGRAM;
    let fits = [FIELDS, BUILT, RETURN]
        .iter()
        .all(|edit| standard.matches(edit).count() == 1)
        && !standard.contains("struct Parameters");
    // `everywhere` lifts the mark for see-through materials, whose alpha is coverage.
    fits.then(|| {
        format!(
            "struct Parameters {{ glow:vec3<f32>, everywhere:f32, bounce:vec3<f32>, mapped:f32 }};\n\
             const DEFAULT_PARAMETERS:Parameters = \
             Parameters(vec3<f32>(0.0),0.0,vec3<f32>(0.0),0.0);\n\
             fn ara_glows(input:SurfaceInput) -> bool {{ return params.everywhere>0.5 || \
             abs(input.material.base_color.a-{GLOW_MARK})<0.002; }}\n\
             {}\n\
             fn emission(s:Surface, input:SurfaceInput) -> vec3<f32> {{ return \
             select(vec3<f32>(0.0),max(params.bounce,vec3<f32>(0.0)),ara_glows(input)); }}\n",
            standard
                .replace(FIELDS, "alpha:f32, glow:vec3<f32> };")
                .replace(
                    BUILT,
                    &format!(
                        "decal_mr.x,alpha,select(vec3<f32>(0.0),\
                         max(params.glow,vec3<f32>(0.0))\
                         *mix(vec3<f32>(1.0),{MAP},clamp(params.mapped,0.0,1.0)),\
                         ara_glows(input)));"
                    ),
                )
                .replace(
                    RETURN,
                    "    return min(exposed+exposed_product(s.glow,vec3<f32>(1.0)),vec3<f32>(65504.0));",
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

/// How a masked draw's geometry is cut to its material's alpha: the base-colour map's
/// alpha times `opacity`, kept where it reaches `cutoff`.
struct Cut<'a> {
    texture: &'a ModelTexture,
    opacity: f32,
    cutoff: f32,
}

/// The cut a draw's material asks for: an alpha mask over a base-colour map, or a
/// blended material whose map is there-or-not (`cutout::Alpha::Binary`) and whose
/// vertices do not fade it, cut at half coverage. Both stay in the opaque path. None
/// for opaque materials and for blended ones that are really see-through (`blends`).
fn cut<'a>(draw: &'a Draw, coverage: &mut Coverage) -> Option<Cut<'a>> {
    let (m, textures) = source(draw)?;
    let texture = textures.get(m.base_color_texture?)?;
    let opacity = m.base_color[3] * draw.color[3];
    let cutoff = match m.alpha_mode {
        1 => m.alpha_cutoff,
        2 if opacity >= 1.
            && coverage.of(texture).class == cutout::Alpha::Binary
            && !coverage.faded(draw) =>
        {
            0.5
        }
        _ => return None,
    };
    Some(Cut {
        texture,
        opacity,
        cutoff,
    })
}

/// Whether a material blends: by an alpha factor below one, by a base-colour map with
/// partial coverage (an opacity map; `cutout::Alpha::Partial`), or by vertex colour
/// alpha. Renderlab's transparent pass takes all three as coverage. A blended material
/// whose map has no alpha and whose vertices do not fade it is opaque, and one whose
/// map is there-or-not is cut (`cut`): transparency costs Renderlab milliseconds a
/// frame, so only what is seen through pays for it.
fn blends(
    draw: &Draw,
    m: &ModelMaterial,
    textures: &[ModelTexture],
    opacity: f32,
    coverage: &mut Coverage,
) -> bool {
    m.alpha_mode == 2
        && (opacity < 1.
            || m.base_color_texture
                .and_then(|index| textures.get(index))
                .is_some_and(|map| coverage.of(map).class == cutout::Alpha::Partial)
            || coverage.faded(draw))
}

/// Whether the draw can be seen through (as `Scene::material` decides): such draws
/// sort per object and never join a static batch.
fn see_through(draw: &Draw, coverage: &mut Coverage) -> bool {
    match source(draw) {
        Some((m, textures)) => {
            m.transmission_factor > 0.
                || blends(draw, m, textures, m.base_color[3] * draw.color[3], coverage)
        }
        None => draw.color[3] < 1.,
    }
}

fn geometry_key(draw: &Draw, coverage: &mut Coverage) -> u64 {
    let hash = match &draw.geometry {
        Geometry::Shape { shape, size } => size.iter().fold(fnv(FNV, &[*shape as u8]), |h, v| {
            fnv(h, &v.to_bits().to_le_bytes())
        }),
        Geometry::Model { model, primitive } => fnv(
            fnv(FNV, &model.digest.0),
            &(*primitive as u64).to_le_bytes(),
        ),
    };
    let hash = fnv(hash, &[u8::from(normal_mapped(draw))]);
    // Cut geometry belongs to its mask as much as to its triangles: the material key
    // names the mask's texture, opacity and cutoff.
    match cut(draw, coverage) {
        Some(_) => fnv(hash, &material_key(draw, coverage).to_le_bytes()),
        None => hash,
    }
}

fn material_key(draw: &Draw, coverage: &mut Coverage) -> u64 {
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
    // Primitives sharing a blended material differ in whether their vertices fade it.
    if source(draw).is_some_and(|(m, _)| m.alpha_mode == 2) {
        hash = fnv(hash, &[u8::from(coverage.faded(draw))]);
    }
    hash
}

pub(crate) fn vertex(v: &ModelVertex) -> Vertex {
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
    /// Triangles of the largest mesh this scene has uploaded (cut meshes and static
    /// batches included). Renderlab's non-RT shadows size their ids by it.
    // On Apple, only the non-RT path caps foliage by it.
    #[cfg_attr(target_vendor = "apple", allow(dead_code))]
    pub fn largest_triangles(&self) -> usize {
        self.largest
    }

    /// Makes Renderlab's objects equal `frame.draws` followed by `static_draws`
    /// (draws the bridge derives itself, such as foliage). `static_key` names the
    /// content of `static_draws`: while it is unchanged, draws already placed are not
    /// looked at again. `reserved` mesh-material groups are in use elsewhere (terrain
    /// tiles) and come off this frame's budget.
    pub fn sync(
        &mut self,
        engine: &mut Engine,
        frame: &RenderFrame,
        static_draws: &[Draw],
        static_key: u64,
        reserved: usize,
        gaps: &mut Gaps,
    ) -> Result<()> {
        // A frame whose draws, foliage and terrain reservation are what the last pass saw makes
        // the same objects, batches and ids: nothing to do (a moving camera, a redraw).
        let seen = Synced {
            draws: draws_signature(frame),
            static_key,
            reserved,
        };
        if self.synced == Some(seen) {
            return Ok(());
        }
        self.synced = None;
        let budget = MAX_GROUPS.saturating_sub(reserved);
        let settled = self.static_key == Some(static_key);
        self.coverage.prune();
        // Each draw's group, and the groups in order of first appearance.
        let mut keyed = Vec::with_capacity(frame.draws.len() + static_draws.len());
        let mut index: HashMap<(MeshKey, u64), usize> = HashMap::new();
        let mut groups: Vec<Group> = Vec::new();
        let mut singular = 0usize;
        let draws = frame
            .draws
            .iter()
            .map(|draw| (draw, false))
            .chain(static_draws.iter().map(|draw| (draw, true)))
            .filter(|(draw, _)| {
                let drawable = drawable(&draw.world);
                singular += usize::from(!drawable);
                drawable
            });
        for (draw, fixed) in draws {
            let placed = self
                .objects
                .get(&draw.entity)
                .filter(|_| fixed && settled)
                .map(|object| (object.mesh, object.material));
            let (mesh, material) = placed.unwrap_or_else(|| {
                let mesh = match draw.skin {
                    Some(_) => MeshKey::Skinned(draw.entity),
                    None => MeshKey::Static(geometry_key(draw, &mut self.coverage)),
                };
                (mesh, material_key(draw, &mut self.coverage))
            });
            let group = *index.entry((mesh, material)).or_insert_with(|| {
                groups.push(Group {
                    material,
                    // Cut geometry comes from its own upload, so it never joins a batch.
                    mergeable: matches!(mesh, MeshKey::Static(_))
                        && !see_through(draw, &mut self.coverage)
                        && cut(draw, &mut self.coverage).is_none(),
                    objects: 0,
                });
                groups.len() - 1
            });
            groups[group].objects += 1;
            keyed.push((draw, mesh, material, placed.is_some(), group));
        }
        let fates = batch::plan(&groups, budget);

        let mut live = HashSet::with_capacity(keyed.len());
        let mut merged: HashMap<u64, Vec<&Draw>> = HashMap::new();
        let mut dropped = 0usize;
        for (draw, mesh, material, placed, group) in keyed {
            match fates[group] {
                Fate::Dropped => {
                    dropped += 1;
                    continue;
                }
                Fate::Merged => {
                    merged.entry(material).or_default().push(draw);
                    continue;
                }
                Fate::Own => {}
            }
            live.insert(draw.entity);
            if placed {
                continue;
            }
            if draw.paint.is_some() {
                gaps.unsupported(
                    "per-instance vertex paint",
                    "the mesh keeps its own colours",
                );
            }
            self.place(engine, frame, draw, mesh, material, gaps)?;
        }
        self.static_key = Some(static_key);
        if singular > 0 {
            gaps.unsupported(
                "draws with a singular transform",
                &format!("{singular} draws (a zero or squashed scale) are not drawn, as in Ara's renderer"),
            );
        }
        if !merged.is_empty() {
            gaps.limit(
                "mesh-material groups per frame",
                &format!(
                    "{} draws are merged into {} static batches to fit the limit of {MAX_GROUPS} ({reserved} of them terrain tiles); a batch whose members move is rebuilt",
                    merged.values().map(Vec::len).sum::<usize>(),
                    merged.len()
                ),
            );
        }
        if dropped > 0 {
            gaps.limit(
                "mesh-material groups after static batching",
                &format!(
                    "{dropped} draws of this frame are not drawn: more distinct materials than the limit of {MAX_GROUPS} ({reserved} of them terrain tiles)"
                ),
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
        self.sync_batches(engine, merged, gaps)?;
        self.tag(engine, frame)?;
        self.synced = Some(seen);
        Ok(())
    }

    /// Gives Renderlab's objects the editor's ids: a draw's id is its index in the frame's
    /// draws plus one (what `RenderFrame::draws` lists, so the editor reads the same
    /// numbers), one range for an own object and one range per member for a static batch.
    /// Draws the frame does not list (foliage) stay untagged. Sets only what changed, since
    /// a draw leaving the list renumbers those after it.
    fn tag(&mut self, engine: &mut Engine, frame: &RenderFrame) -> Result<()> {
        let ids: HashMap<u64, u32> = frame
            .draws
            .iter()
            .enumerate()
            .map(|(n, draw)| (draw.entity, n as u32 + 1))
            .collect();
        let mut wanted = Tags::new();
        for (entity, object) in &self.objects {
            if let Some(id) = ids.get(entity) {
                wanted.insert((false, *entity), (object.id, vec![(0, u32::MAX, *id)]));
            }
        }
        for (material, batch) in &self.batches {
            let mut first = 0u32;
            let mut ranges = Vec::new();
            for (entity, triangles) in &batch.layout {
                if let (Some(id), true) = (ids.get(entity), *triangles > 0) {
                    ranges.push((first, *triangles, *id));
                }
                first += triangles;
            }
            wanted.insert((true, *material), (batch.id, ranges));
        }
        self.tags.retain(|key, _| wanted.contains_key(key));
        for (key, tagged) in wanted {
            if self.tags.get(&key) != Some(&tagged) {
                engine.set_object_ids(tagged.0, &tagged.1)?;
                self.tags.insert(key, tagged);
            }
        }
        Ok(())
    }

    /// Makes the static batches equal `merged` (members by material key): a batch
    /// whose members or their transforms changed is rebuilt, one no longer needed
    /// is removed.
    fn sync_batches(
        &mut self,
        engine: &mut Engine,
        merged: HashMap<u64, Vec<&Draw>>,
        gaps: &mut Gaps,
    ) -> Result<()> {
        let gone: Vec<u64> = self
            .batches
            .keys()
            .filter(|material| !merged.contains_key(material))
            .copied()
            .collect();
        for material in gone {
            self.remove_batch(engine, material)?;
        }
        for (material, members) in merged {
            let content = members
                .iter()
                .fold(fnv(FNV, &material.to_le_bytes()), |h, draw| {
                    let h = fnv(h, &geometry_key(draw, &mut self.coverage).to_le_bytes());
                    let h = fnv(h, &draw.entity.to_le_bytes());
                    cleaned(&draw.world)
                        .to_cols_array()
                        .iter()
                        .fold(h, |h, v| fnv(h, &v.to_bits().to_le_bytes()))
                });
            if self
                .batches
                .get(&material)
                .is_some_and(|b| b.content == content)
            {
                continue;
            }
            self.remove_batch(engine, material)?;
            let first = members[0];
            let wanted = normal_mapped(first);
            let (mut vertices, mut indices) = (Vec::new(), Vec::new());
            let mut layout = Vec::with_capacity(members.len());
            for draw in &members {
                let (mut source, mut triangles) = triangles(draw)?;
                if wanted {
                    split_handedness(source.to_mut(), triangles.to_mut());
                }
                let before = indices.len();
                batch::append_world(
                    &cleaned(&draw.world),
                    &source,
                    &triangles,
                    &mut vertices,
                    &mut indices,
                );
                layout.push((draw.entity, ((indices.len() - before) / 3) as u32));
            }
            let label = format!(
                "a static batch of {} draws like `{}`",
                members.len(),
                first.label
            );
            self.largest = self.largest.max(indices.len() / 3);
            let (mesh, tangents) = upload(engine, &label, &vertices, &indices, wanted, gaps)?;
            let material_slot = fnv(material, &[u8::from(tangents)]);
            let material_id = self.material(engine, first, material_slot, tangents, gaps)?;
            let id = engine.spawn(mesh, material_id, Mat4::IDENTITY)?;
            self.batches.insert(
                material,
                Batch {
                    content,
                    id,
                    mesh,
                    material_slot,
                    layout,
                },
            );
        }
        Ok(())
    }

    /// Despawns a material's static batch and releases its mesh and its use of the material.
    fn remove_batch(&mut self, engine: &mut Engine, material: u64) -> Result<()> {
        let Some(batch) = self.batches.remove(&material) else {
            return Ok(());
        };
        engine.despawn(batch.id)?;
        engine.remove_mesh(batch.mesh)?;
        self.release_material(engine, batch.material_slot)
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
        let world = cleaned(&draw.world).to_cols_array();
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
                    let (id, tangents) =
                        skinned_mesh(engine, draw, joints, &mut self.coverage, gaps)?;
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
        self.release_material(engine, object.material_slot)
    }

    /// Drops one use of a shared material, removing it with its last user.
    fn release_material(&mut self, engine: &mut Engine, slot: u64) -> Result<()> {
        if let Some(shared) = self.materials.get_mut(&slot) {
            shared.users -= 1;
            if shared.users == 0 {
                engine.remove_material(shared.id)?;
                self.materials.remove(&slot);
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
        let label = format!("`{}`", draw.label);
        // A mask that hides nothing is no cut: the mesh uploads as it is, below.
        if let Some(c) = cut(draw, &mut self.coverage)
            && let Some(cut) = cut_mesh(engine, &label, &source, &indices, &c, wanted, gaps)?
        {
            self.largest = self.largest.max(cut.triangles);
            self.meshes.insert(
                key,
                (
                    Shared {
                        id: cut.id,
                        users: 1,
                    },
                    cut.tangents,
                ),
            );
            return Ok((cut.id, cut.tangents));
        }
        if wanted {
            split_handedness(source.to_mut(), indices.to_mut());
        }
        self.largest = self.largest.max(indices.len() / 3);
        let (id, tangents) = upload(engine, &label, &source, &indices, wanted, gaps)?;
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
        // The emissive map, with the base-colour and metallic-roughness textures that
        // will carry it, and whether it has texture coordinates of its own.
        let mut emissive_map = None;
        let mut own_coordinates = false;
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
            if let Some(map) = m.emissive_texture.and_then(|index| textures.get(index)) {
                let host = |slot: Option<usize>| slot.and_then(|index| textures.get(index));
                let (base, data) = (
                    host(m.base_color_texture),
                    host(m.metallic_roughness_texture),
                );
                own_coordinates = (base.is_some() && m.emissive_uv != m.base_color_uv)
                    || (data.is_some() && m.emissive_uv != m.metallic_roughness_uv);
                emissive_map = Some((map, base, data));
            }
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
            // A blended material is see-through by its alpha factor or by a base-colour
            // map with partial coverage (`blends`). An alpha mask, or a blended map
            // that is there-or-not, is cut into the geometry (`cut`); the material
            // itself is then opaque.
            blend = blends(draw, m, textures, desc.base_color[3], &mut self.coverage);
            // A material that blends is not cut or drawn opaque, so it guessed nothing.
            if !blend {
                self.report_alpha_guesses(draw, m, textures, gaps);
            }
            if !blend && m.base_color_texture.is_some() {
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
        if blend && desc.transparency.is_none() {
            desc.transparency = Some(Transparency {
                opacity: 1.,
                ior: 1.,
                thickness: 0.,
            });
        }
        if glow.iter().any(|c| *c > 0.) {
            // The map rides in channels only an opaque material leaves unused, sampled
            // with the textures that carry it.
            let opaque = desc.transparency.is_none();
            let packable = opaque && !own_coordinates;
            // Only this material glows, by its mark (`GLOW_MARK`).
            let everywhere = Value::F32(if opaque { 0. } else { 1. });
            if opaque {
                desc.base_color[3] = GLOW_MARK;
            }
            match emissive_map {
                Some(_) if !packable => gaps.unsupported(
                    "emissive maps on see-through materials or with their own texture coordinates",
                    "those materials do not emit or glow",
                ),
                Some((map, base, data)) => {
                    if let Some(program) = self.emissive_program(engine, gaps) {
                        let (base, data, mean) = self.glow_textures(engine, map, base, data)?;
                        desc.base_color_texture = Some(base);
                        desc.metallic_roughness_texture = Some(data);
                        let bounce = std::array::from_fn(|i| glow[i] * mean[i]);
                        parameters = [
                            ("glow", Value::Vec3(glow)),
                            ("bounce", Value::Vec3(bounce)),
                            ("everywhere", everywhere),
                            ("mapped", Value::F32(1.)),
                        ];
                        desc.program = Some(program);
                        desc.parameters = &parameters;
                    }
                }
                None => {
                    if let Some(program) = self.emissive_program(engine, gaps) {
                        parameters = [
                            ("glow", Value::Vec3(glow)),
                            ("bounce", Value::Vec3(glow)),
                            ("everywhere", everywhere),
                            ("mapped", Value::F32(0.)),
                        ];
                        desc.program = Some(program);
                        desc.parameters = &parameters;
                    }
                }
            }
        }
        let id = engine.create_material(&desc)?;
        self.materials.insert(key, Shared { id, users: 1 });
        Ok(id)
    }

    /// Says so, naming the material, when a blended material's alpha was guessed: its
    /// map did not decode and is drawn opaque, or the map was classed there-or-not
    /// although more than `PARTLY_REPORTED` of its visible texels are partly there, so
    /// they are cut or kept at half coverage. (The gap report names the first such
    /// material.)
    fn report_alpha_guesses(
        &mut self,
        draw: &Draw,
        m: &ModelMaterial,
        textures: &[ModelTexture],
        gaps: &mut Gaps,
    ) {
        if m.alpha_mode != 2 {
            return;
        }
        let Some(map) = m.base_color_texture.and_then(|index| textures.get(index)) else {
            return;
        };
        let alpha = self.coverage.of(map);
        if !alpha.decoded {
            gaps.unsupported(
                "blended materials whose base-colour map does not decode",
                &format!(
                    "`{}` is drawn opaque (only BC7 and BC5 maps are read)",
                    draw.label
                ),
            );
        } else if alpha.class == cutout::Alpha::Binary && alpha.partly > PARTLY_REPORTED {
            gaps.unsupported(
                "partly transparent texels in a blended map that is mostly there-or-not",
                &format!(
                    "`{}` is cut at half coverage; {:.0}% of its visible texels are partly transparent and are cut or kept",
                    draw.label,
                    alpha.partly * 100.
                ),
            );
        }
    }

    /// The program emissive materials shade with, with or without an emissive map,
    /// composed on first use; None (and one report) when Renderlab rejects it.
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

    /// The base-colour and metallic-roughness textures of a material with emissive
    /// map `glow`: its own (`base`, `data`) or white, each carrying part of the map
    /// (`GLOW_IN_BASE`, `GLOW_IN_DATA`), and the map's mean linear colour. Uploaded
    /// once per combination.
    fn glow_textures(
        &mut self,
        engine: &mut Engine,
        glow: &ModelTexture,
        base: Option<&ModelTexture>,
        data: Option<&ModelTexture>,
    ) -> Result<(TextureRef, TextureRef, [f32; 3])> {
        let glow_key = fnv(FNV, &glow.ktx2);
        let key = |host: Option<&ModelTexture>, tag: u8| {
            let host = host.map_or(0, |host| fnv(FNV, &host.ktx2));
            (fnv(glow_key, &host.to_le_bytes()), tag)
        };
        let (base_key, data_key) = (key(base, 3), key(data, 4));
        let known = self.glow_means.contains_key(&glow_key)
            && self.textures.contains_key(&base_key)
            && self.textures.contains_key(&data_key);
        if !known {
            let decoded = ara_runtime_render::decode_texture(&glow.ktx2)?;
            self.glow_means
                .insert(glow_key, texture::mean_linear(&decoded.rgba));
            for (key, host, role, channels) in [
                (
                    base_key,
                    base.map(|host| &host.ktx2[..]),
                    TextureRole::BaseColorSrgb,
                    &GLOW_IN_BASE[..],
                ),
                (
                    data_key,
                    data.map(|host| &host.ktx2[..]),
                    TextureRole::MetallicRoughnessLinear,
                    &GLOW_IN_DATA[..],
                ),
            ] {
                if let Entry::Vacant(slot) = self.textures.entry(key) {
                    let glow = (decoded.size, decoded.rgba.as_slice());
                    let packed = texture::with_glow(host, glow, role, channels)?;
                    slot.insert(engine.create_texture(packed)?);
                }
            }
        }
        let bound = |texture: TextureId| TextureRef {
            texture,
            sampler: Sampler::default(),
        };
        Ok((
            bound(self.textures[&base_key]),
            bound(self.textures[&data_key]),
            self.glow_means[&glow_key],
        ))
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

/// Uploads static triangles, with tangents when `wanted` (a normal-mapped material) and
/// usable; otherwise without, reported under `label`. Returns the mesh and whether it
/// carries tangents.
fn upload(
    engine: &mut Engine,
    label: &str,
    source: &[ModelVertex],
    indices: &[u32],
    wanted: bool,
    gaps: &mut Gaps,
) -> Result<(MeshId, bool)> {
    let vertices: Vec<Vertex> = source.iter().map(vertex).collect();
    let data = MeshData {
        vertices: &vertices,
        indices,
    };
    let frames = wanted.then(|| tangents(source)).flatten();
    let uploaded = frames
        .as_ref()
        .map(|frames| engine.create_mesh_with_tangents(&data, frames));
    Ok(match uploaded {
        Some(Ok(id)) => (id, true),
        rejected => {
            if wanted {
                let why = match rejected {
                    Some(Err(error)) => error.to_string(),
                    _ => "a tangent is missing or lies along its normal".to_owned(),
                };
                gaps.unsupported(
                    "normal maps on meshes without usable tangents",
                    &format!("{label} shades with its vertex normals ({why})"),
                );
            }
            (engine.create_mesh(&data)?, false)
        }
    })
}

/// Contour cells a mesh's cut may visit in all: its uv area times the cells per uv unit,
/// squared. Bounds the cut's triangles on meshes whose uvs tile many times (fences).
const CUT_CELLS: f64 = 65_536.;

/// A mesh uploaded cut to its mask.
struct CutMesh {
    id: MeshId,
    triangles: usize,
    /// It carries tangent frames (`cutout::tangents`): its material may sample a normal map.
    tangents: bool,
}

/// Uploads `source` cut to its material's alpha (`Cut`): Renderlab keeps the triangles
/// where the mask passes, with new edges along the contour. A normal-mapped material
/// gets tangent frames derived from the cut mesh's own uv mapping; where those cannot
/// be derived the normal map is dropped, reported under `label`. None when the mask
/// hides nothing: the caller uploads the mesh as it is.
fn cut_mesh(
    engine: &mut Engine,
    label: &str,
    source: &[ModelVertex],
    indices: &[u32],
    c: &Cut<'_>,
    normal_mapped: bool,
    gaps: &mut Gaps,
) -> Result<Option<CutMesh>> {
    let decoded = ara_runtime_render::decode_texture(&c.texture.ktx2)?;
    let [width, height] = decoded.size;
    let vertices: Vec<Vertex> = source.iter().map(vertex).collect();
    if cutout::hides_nothing(&decoded.rgba, &vertices, c.opacity, c.cutoff) {
        return Ok(None);
    }
    let data = MeshData {
        vertices: &vertices,
        indices,
    };
    // The mask at the grid's scale (`cutout::coverage`), then Renderlab's cut.
    let cut_at = |cells: u32| {
        let texels_per_cell = width.max(height) / cells.max(1);
        let mask = cutout::coverage(decoded.size, &decoded.rgba, texels_per_cell);
        renderlab::engine::cutout(&data, &mask, c.opacity, c.cutoff, cells)
    };
    // About eight texels per contour cell, within the 32 to 128 cells per uv unit that
    // foliage cards take, and coarser where the uvs cover many units (`CUT_CELLS`).
    let uv_area: f64 = indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|t| {
            let [a, b, c] = t.map(|i| source.get(i as usize).map(|v| v.uv));
            let ([ax, ay], [bx, by], [cx, cy]) = (a?, b?, c?);
            Some(0.5 * f64::from(((bx - ax) * (cy - ay) - (cx - ax) * (by - ay)).abs()))
        })
        .sum();
    let fine = (width.max(height) / 8).clamp(32, 128);
    let bounded = (CUT_CELLS / uv_area.max(1e-6)).sqrt().clamp(4., 128.) as u32;
    let mut cells = fine.min(bounded);
    // Dense meshes split along every cell line: coarser cells until the cut holds at
    // most four times its source triangles (at least 8,192). Foliage cards repeat by
    // the thousand, and every triangle costs the non-RT shadow pass.
    let allowed = (indices.len() / 3 * 4).max(8_192);
    let mut cut = cut_at(cells)?;
    while cut.indices.len() / 3 > allowed && cells > 4 {
        cells = (cells / 2).max(4);
        cut = cut_at(cells)?;
    }
    let triangles = cut.indices.len() / 3;
    let frames = normal_mapped
        .then(|| cutout::tangents(&cut.vertices, &cut.indices))
        .flatten();
    let uploaded = frames
        .as_ref()
        .map(|frames| engine.create_mesh_with_tangents(&cut.data(), frames));
    let (id, tangents) = match uploaded {
        Some(Ok(id)) => (id, true),
        rejected => {
            if normal_mapped {
                let why = match rejected {
                    Some(Err(error)) => error.to_string(),
                    _ => "its uv mapping gives no consistent tangents".to_owned(),
                };
                gaps.unsupported(
                    "normal maps on alpha-cut meshes without derivable tangents",
                    &format!("{label} shades with its vertex normals ({why})"),
                );
            }
            (engine.create_mesh(&cut.data())?, false)
        }
    };
    Ok(Some(CutMesh {
        id,
        triangles,
        tangents,
    }))
}

/// Uploads one skinned primitive with its own pose (`joints` plus the identity joint).
/// Returns the mesh and whether it carries tangents.
fn skinned_mesh(
    engine: &mut Engine,
    draw: &Draw,
    joints: usize,
    coverage: &mut Coverage,
    gaps: &mut Gaps,
) -> Result<(MeshId, bool)> {
    if cut(draw, coverage).is_some() {
        gaps.unsupported(
            "alpha masks on skinned meshes",
            "those primitives draw opaque, without cutouts",
        );
    }
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
    /// Transforms Renderlab can light draw; a zero or squashed scale, a non-finite one or axes
    /// collapsed onto a plane do not.
    #[test]
    fn singular_transforms_are_not_drawn() {
        use glam34::{Mat4, Vec3};
        assert!(drawable(&Mat4::IDENTITY));
        assert!(drawable(&Mat4::from_scale(Vec3::new(24., 0.01, 24.))));
        assert!(!drawable(&Mat4::from_scale(Vec3::ZERO)));
        assert!(!drawable(&Mat4::from_scale(Vec3::new(1., 1., 0.))));
        assert!(!drawable(&Mat4::from_scale(Vec3::new(1., 1., 1e-5))));
        assert!(drawable(&Mat4::from_scale(Vec3::new(1e4, 1., 1.))));
        assert!(!drawable(&Mat4::from_scale(Vec3::new(1e4 + 1., 1., 1.))));
        let mut rounded = Mat4::from_translation(Vec3::new(61., 0., 77.));
        rounded.w_axis.w = 0.999_999_94;
        assert!(
            drawable(&rounded),
            "an f32 composition error in the last row is repaired"
        );
        assert_eq!(cleaned(&rounded).w_axis.w, 1.);
        let mut projective = Mat4::IDENTITY;
        projective.z_axis.w = 0.5;
        assert!(!drawable(&projective), "a real projective part is not");
        assert!(!drawable(&Mat4::from_translation(Vec3::new(
            f32::NAN,
            0.,
            0.
        ))));
        let flat = Mat4::from_cols_array(&[
            1., 0., 0., 0., 2., 0., 0., 0., 3., 0., 0., 0., 0., 0., 0., 1.,
        ]);
        assert!(!drawable(&flat));
    }

    use super::*;
    use ara_asset_format::{TextureFormat, write_ktx2};
    use ara_runtime::{model::Model, model::Primitive, model::TextureUsage};
    use std::{collections::BTreeMap, sync::Arc};

    /// One BC7 block of a solid colour (mode 6, all texels the same).
    fn bc7_solid(rgba: [u8; 4]) -> [u8; 16] {
        let parity = u128::from(rgba[3] & 1);
        let mut bits = 1u128 << 6;
        for (channel, value) in rgba.into_iter().enumerate() {
            let endpoint = u128::from(value >> 1);
            bits |= endpoint << (7 + 14 * channel) | endpoint << (14 + 14 * channel);
        }
        (bits | parity << 63 | parity << 64).to_le_bytes()
    }

    /// An 8x8 base-colour map whose left half is opaque and right half clear: a
    /// there-or-not alpha (`cutout::Alpha::Binary`).
    fn leaf_map() -> ModelTexture {
        let blocks = [[20, 220, 20, 255], [20, 220, 20, 0]]
            .repeat(2)
            .into_iter()
            .map(bc7_solid)
            .collect::<Vec<_>>()
            .concat();
        ModelTexture {
            name: "leaf".into(),
            usage: TextureUsage::Color,
            ktx2: write_ktx2(8, 8, TextureFormat::Bc7Srgb, &[blocks])
                .expect("a valid KTX2 container")
                .into(),
        }
    }

    /// One model with a one-triangle primitive per entry of `alphas`, the vertex colour alpha
    /// of that primitive, all on the same BLEND material (slot 0) over `leaf_map`.
    fn leaf_model(alphas: &[u8]) -> Arc<Model> {
        Arc::new(Model {
            asset: "leaf".into(),
            digest: ara_asset::Digest::of(alphas),
            primitives: alphas
                .iter()
                .map(|&alpha| Primitive {
                    vertices: vec![
                        ModelVertex {
                            color: [255, 255, 255, alpha],
                            ..Default::default()
                        };
                        3
                    ],
                    indices: vec![0, 1, 2],
                    lods: Vec::new(),
                    material: Some(0),
                    skin: None,
                    casts_shadows: true,
                    source_vertices: Vec::new(),
                    vertex_colors: alpha != 255,
                })
                .collect(),
            nodes: BTreeMap::new(),
            node_names: BTreeMap::new(),
            materials: vec![ModelMaterial {
                alpha_mode: 2,
                base_color_texture: Some(0),
                ..Default::default()
            }],
            textures: vec![leaf_map()],
            skins: Vec::new(),
            bounds: [[0.; 3]; 2],
        })
    }

    /// A draw of primitive `primitive` of `model`.
    fn leaf_draw(model: &Arc<Model>, primitive: usize) -> Draw {
        Draw {
            entity: 1,
            address: "e_1".into(),
            label: "leaf".into(),
            geometry: Geometry::Model {
                model: Arc::clone(model),
                primitive,
            },
            world: glam34::Mat4::IDENTITY,
            color: [1.; 4],
            roughness: 0.6,
            metallic: 0.,
            emissive: [0.; 3],
            emissive_factor: None,
            material: None,
            skin: None,
            casts_shadows: true,
            receives_decals: true,
            paint: None,
        }
    }

    /// The skip key follows everything a draw is made from, and nothing else.
    #[test]
    fn the_draws_signature_changes_with_a_draw_and_not_with_a_redraw() {
        let model = leaf_model(&[255]);
        let frame_of = |draws: Vec<Draw>| {
            let mut frame = RenderFrame::default();
            frame.draws = draws;
            frame
        };
        let one = || leaf_draw(&model, 0);
        let before = draws_signature(&frame_of(vec![one()]));
        // Another tick or camera is not a change of the draws.
        let mut redrawn = frame_of(vec![one()]);
        redrawn.tick += 1;
        redrawn.alpha = 0.5;
        assert_eq!(draws_signature(&redrawn), before);
        // A moved, recoloured, added or removed draw is.
        let mut moved = one();
        moved.world = glam34::Mat4::from_translation(glam34::Vec3::X);
        assert_ne!(draws_signature(&frame_of(vec![moved])), before);
        let mut tinted = one();
        tinted.color = [0.5; 4];
        assert_ne!(draws_signature(&frame_of(vec![tinted])), before);
        assert_ne!(draws_signature(&frame_of(vec![one(), one()])), before);
        assert_ne!(draws_signature(&frame_of(Vec::new())), before);
    }

    #[test]
    fn a_blend_primitive_fading_by_vertex_alpha_blends_and_is_not_cut() {
        let mut coverage = Coverage::default();
        // One model, one material slot, two primitives: one with solid vertices, one fading.
        let model = leaf_model(&[255, 100]);
        // Solid vertices over a there-or-not map: a cut-out in the opaque path.
        let solid = leaf_draw(&model, 0);
        let (m, textures) = source(&solid).expect("a model material");
        assert!(!blends(&solid, m, textures, 1., &mut coverage));
        assert!(!see_through(&solid, &mut coverage));
        assert_eq!(cut(&solid, &mut coverage).map(|c| c.cutoff), Some(0.5));
        // The same map under vertices that fade: Renderlab's transparent pass
        // multiplies coverage by vertex alpha, so it blends and is not cut.
        let faded = leaf_draw(&model, 1);
        let (m, textures) = source(&faded).expect("a model material");
        assert!(blends(&faded, m, textures, 1., &mut coverage));
        assert!(see_through(&faded, &mut coverage));
        assert!(cut(&faded, &mut coverage).is_none());
        // Primitives of one model sharing a material slot but not its fade get different
        // materials: the two keys differ only by the fade, not by model or slot.
        assert_ne!(
            material_key(&solid, &mut coverage),
            material_key(&faded, &mut coverage)
        );
    }

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
        // The one program reads the map from the material's textures only as far as
        // its `mapped` parameter says.
        assert!(source.contains("mr.r,mr.a,sampled.a"));
        assert!(source.contains("clamp(params.mapped,0.0,1.0)"));
        // Global illumination gets the averaged glow, never a texture lookup, and the
        // program glows only on the marked material.
        assert!(source.contains("max(params.bounce,vec3<f32>(0.0)),ara_glows(input))"));
        assert!(source.contains("base_color.a-0.737)<0.002"));
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
