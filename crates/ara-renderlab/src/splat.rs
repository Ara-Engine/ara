//! Terrain layers on Renderlab, as Ara's renderer shades them (`terrainSurface`,
//! shaders/native-raster.slang): up to four layers' maps in 2x2 atlases shared by every
//! tile, each tile's splat weights in its metallic-roughness slot, and a program
//! (Renderlab's standard program with its `surface` replaced) that blends the layers.
//!
//! - Colour: each layer's base-colour map (the colour atlas, sRGB), repeating over the
//!   ground in world metres by its uv transform, flattened against its own tile-scale
//!   mean and under Ara's world-space breakup noise. Past about 26 degrees the side
//!   projections (world Z/Y and X/Y) fade in, as Ara's do within its detail distance.
//! - Roughness: the layer's metal/roughness map's green in the colour atlas's alpha, near
//!   the camera; its mean farther away; times the roughness factor. Metalness uses the
//!   map's mean times the factor everywhere.
//! - Normals: each layer's normal map (the normal atlas, in Renderlab's normal slot,
//!   top projection), faded out over the detail distance and on steep slopes, blended in
//!   the splat's tangent frame (+X, +Z for world X/Z coordinates).
//!
//! The detail distance is the user's graphics tier (`ExternalView::terrain_detail`).
use crate::rl::Engine;
use crate::{gaps::Gaps, texture};
use anyhow::Result;
use ara_runtime::model::ModelMaterial;
use renderlab::{
    asset::TextureRole,
    engine::{ProgramId, TextureId, Value},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Side of one layer's quadrant in an atlas, texels.
const LAYER_SIZE: u32 = 512;
/// The quadrant mip Ara flattens against: 8x8 texels per layer (log2(512) - 3).
const TILE_LEVEL: u32 = 6;

/// The splat program's `struct Parameters` and the `surface` that reads it.
const SURFACE: &str = r#"
// Renderlab allows 256 bytes of parameters. uv: each layer's maps' repeat per metre (xy)
// and offset (zw), shared by its colour, roughness and normal maps; tint: base colour
// factor (rgb) and roughness factor (a); layer: x metallic factor times its map's mean,
// y the colour map's mean luminance (0: no map, not flattened), z the roughness map's
// mean (1: none; the colour atlas's alpha holds the map, or that mean where it tiles
// otherwise), w the normal-map scale (0: no normal map); detail: x the detail distance in
// metres (0: no detail maps or side projections).
struct Parameters { uv:array<vec4<f32>,4>, tint:array<vec4<f32>,4>, layer:array<vec4<f32>,4>, detail:vec4<f32> };
const DEFAULT_PARAMETERS:Parameters=Parameters(
    array<vec4<f32>,4>(vec4<f32>(1.0,1.0,0.0,0.0),vec4<f32>(1.0,1.0,0.0,0.0),vec4<f32>(1.0,1.0,0.0,0.0),vec4<f32>(1.0,1.0,0.0,0.0)),
    array<vec4<f32>,4>(vec4<f32>(1.0,1.0,1.0,0.5),vec4<f32>(1.0,1.0,1.0,0.5),vec4<f32>(1.0,1.0,1.0,0.5),vec4<f32>(1.0,1.0,1.0,0.5)),
    array<vec4<f32>,4>(vec4<f32>(0.0,0.0,1.0,0.0),vec4<f32>(0.0,0.0,1.0,0.0),vec4<f32>(0.0,0.0,1.0,0.0),vec4<f32>(0.0,0.0,1.0,0.0)),
    vec4<f32>(0.0));
// Layer i's quadrant of an atlas at repeating coordinates `uv` (layer units, with their
// one-pixel footprints), kept half a texel of the sampled level inside the quadrant so
// its neighbours never bleed in.
fn ara_atlas(input:SurfaceInput, i:u32, uv:vec2<f32>, uv_dx:vec2<f32>, uv_dy:vec2<f32>, normal:bool) -> vec4<f32> {
    let dx=uv_dx*0.5;
    let dy=uv_dy*0.5;
    let texels=max(length(dx),length(dy))*ATLAS_SIZE;
    let margin=clamp(0.5*max(texels,1.0)/(0.5*ATLAS_SIZE),0.0,0.25);
    let cell=vec2<f32>(f32(i&1u),f32(i>>1u));
    let at=(cell+clamp(fract(uv),vec2<f32>(margin),vec2<f32>(1.0-margin)))*0.5;
    if normal { return sample_normal_layer_grad(input,at,dx,dy,0); }
    return sample_base_color_grad(input,at,dx,dy);
}
fn ara_luminance(c:vec3<f32>) -> f32 { return dot(c,vec3<f32>(0.2126,0.7152,0.0722)); }
// Layer i's colour (rgb, flattened as Ara's terrainFlat) and roughness-map value (a) in
// one projection: world coordinates `p` and their footprints.
fn ara_colour(input:SurfaceInput, i:u32, p:vec2<f32>, p_dx:vec2<f32>, p_dy:vec2<f32>) -> vec4<f32> {
    let t=params.uv[i];
    let uv=p*t.xy+t.zw;
    var c=ara_atlas(input,i,uv,p_dx*t.xy,p_dy*t.xy,false);
    let mean=params.layer[i].y;
    if mean>0.0 {
        // The quadrant's 8x8 level: an isotropic footprint of 2^TILE_LEVEL texels.
        let tile=vec2<f32>(TILE_FOOTPRINT,0.0);
        let local=ara_luminance(ara_atlas(input,i,uv,tile,tile.yx,false).rgb);
        c=vec4<f32>(c.rgb*clamp(mean/max(local,1e-4),0.5,2.0),c.a);
    }
    return c;
}
// Projection weights from the surface normal: the top projection alone on gentle
// slopes; past about 26 degrees the world Z/Y and X/Y side projections fade in.
fn ara_projection(n:vec3<f32>) -> vec3<f32> {
    var w=pow(abs(n),vec3<f32>(4.0));
    w/=dot(w,vec3<f32>(1.0));
    let side=clamp((w.x+w.z-0.05)*10.0,0.0,1.0);
    w.x*=side; w.z*=side; w.y=1.0-w.x-w.z;
    return w;
}
fn ara_hash(p:vec2<i32>) -> f32 {
    var h=(bitcast<u32>(p.x)*0x8da6b343u)^(bitcast<u32>(p.y)*0xd8163841u);
    h=(h^(h>>16u))*0x7feb352du;
    h^=h>>15u;
    return f32(h&0xffffffu)/16777216.0;
}
fn ara_noise(p:vec2<f32>) -> f32 {
    let i=floor(p);
    var t=p-i;
    t=t*t*(vec2<f32>(3.0)-2.0*t);
    let c=vec2<i32>(i);
    return mix(mix(ara_hash(c),ara_hash(c+vec2<i32>(1,0)),t.x),mix(ara_hash(c+vec2<i32>(0,1)),ara_hash(c+vec2<i32>(1,1)),t.x),t.y);
}
fn surface(input:SurfaceInput) -> Surface {
    let raw=sample_metallic_roughness_grad(input,input.uv,input.uv_dx,input.uv_dy);
    let w=raw/max(raw.x+raw.y+raw.z+raw.w,1e-4);
    var normal=unit_normal(input.normal);
    if dot(normal,normal)<0.5 { normal=input.geometric; }
    let range=params.detail.x;
    let detail=select(0.0,1.0-clamp((length(input.world-input.eye)-range)/(0.25*range),0.0,1.0),range>0.0);
    let projection=ara_projection(normal);
    let sides=range>0.0 && projection.x+projection.z>0.0;
    var base=vec3<f32>(0.0);
    var rough=0.0;
    var tangent_normal=vec3<f32>(0.0);
    for(var i=0u;i<4u;i+=1u) {
        if w[i]<=0.0 { continue; }
        var c=ara_colour(input,i,input.world.xz,input.world_dx.xz,input.world_dy.xz);
        if sides {
            c=c*projection.y
                +ara_colour(input,i,input.world.zy,input.world_dx.zy,input.world_dy.zy)*projection.x
                +ara_colour(input,i,input.world.xy,input.world_dx.xy,input.world_dy.xy)*projection.z;
        }
        base+=w[i]*max(c.rgb*params.tint[i].rgb,vec3<f32>(0.0));
        let layer=params.layer[i];
        rough+=w[i]*clamp(params.tint[i].a*mix(layer.z,c.a,detail),0.045,1.0);
        var n=vec3<f32>(0.0,0.0,1.0);
        if layer.w!=0.0 && detail*projection.y>0.0 {
            let t=params.uv[i];
            let s=ara_atlas(input,i,input.world.xz*t.xy+t.zw,input.world_dx.xz*t.xy,input.world_dy.xz*t.xy,true).xyz*2.0-vec3<f32>(1.0);
            n=mix(n,vec3<f32>(s.xy*layer.w,s.z),detail*projection.y);
        }
        tangent_normal+=w[i]*n;
    }
    // Ara's tile breakup: a smooth world-space brightness variation (31 m and 11 m).
    base*=mix(0.8,1.2,ara_noise(input.world.xz/31.0)*0.65+ara_noise(input.world.xz/11.0+vec2<f32>(17.0))*0.35);
    let metal=clamp(dot(w,vec4<f32>(params.layer[0].x,params.layer[1].x,params.layer[2].x,params.layer[3].x)),0.0,1.0);
    // The splat's tangent frame: +X along the surface, the bitangent toward +Z.
    var t=unit_normal(input.tangent-normal*dot(normal,input.tangent));
    if dot(t,t)<0.5 { t=unit_normal(vec3<f32>(1.0,0.0,0.0)-normal*normal.x); }
    var n=unit_normal(t*tangent_normal.x+input.handedness*cross(normal,t)*tangent_normal.y+normal*tangent_normal.z);
    if dot(n,n)<0.5 { n=normal; }
    // As material_normal: a source-back face of a double-sided surface reverses its
    // frame, then projected decals perturb it.
    if input.material.double_sided!=0u && !input.source_front { n=-n; }
    if input.decals.normal_multiply!=1.0 {
        let decaled=unit_normal(n*input.decals.normal_multiply+input.decals.normal_add);
        n=select(n,decaled,dot(decaled,decaled)>0.5);
    }
    let decal_base=decal_base_color(input,vec4<f32>(clamp(base,vec3<f32>(0.0),vec3<f32>(1.0)),1.0));
    let decal_mr=decal_metallic_roughness(input,vec2<f32>(clamp(rough,0.045,1.0),metal));
    return Surface(n,decal_base.rgb,decal_mr.y,decal_mr.x,1.0);
}
"#;

/// The splat program's source: the standard program with this `surface`. None when the
/// standard program no longer has the shape this edit expects.
pub(crate) fn source() -> Option<String> {
    let standard = renderlab::STANDARD_PROGRAM;
    const START: &str = "fn surface(input:SurfaceInput) -> Surface {";
    let start = standard.find(START)?;
    let end = start + standard[start..].find("\n}\n")? + 3;
    (standard.matches(START).count() == 1 && !standard.contains("struct Parameters")).then(|| {
        format!(
            "const ATLAS_SIZE:f32={}.0;\nconst TILE_FOOTPRINT:f32={};\n{}{SURFACE}{}",
            2 * LAYER_SIZE,
            // A one-pixel footprint of 2^TILE_LEVEL texels, in layer units.
            f64::from(1u32 << TILE_LEVEL) / f64::from(LAYER_SIZE),
            &standard[..start],
            &standard[end..]
        )
    })
}

/// A layer's cooked map and its cache key (the material asset's digest and the slot).
pub(crate) type Map<'a> = (u64, &'a [u8]);

/// One terrain layer's material and its cooked maps.
pub(crate) struct LayerMaps<'a> {
    pub material: &'a ModelMaterial,
    pub color: Option<Map<'a>>,
    pub normal: Option<Map<'a>>,
    pub metal_roughness: Option<Map<'a>>,
    /// Occlusion or emission maps, which Ara's terrain samples and this does not.
    pub unsampled: bool,
}

/// The resolved layers of a terrain: the atlases and the program's parameters.
pub(crate) struct Splat {
    pub atlas: TextureId,
    /// The normal atlas, when any layer has a normal map.
    pub normals: Option<TextureId>,
    pub parameters: Vec<(&'static str, Value)>,
}

impl Splat {
    /// The textures the splat owns, to remove with it.
    pub fn textures(&self) -> impl Iterator<Item = TextureId> {
        std::iter::once(self.atlas).chain(self.normals)
    }
}

/// A decoded map resampled to one atlas quadrant (RGBA8, `LAYER_SIZE` square).
fn quadrant(ktx2: &[u8]) -> Result<Vec<u8>> {
    let decoded = ara_runtime_render::decode_texture(ktx2)?;
    let mut out = vec![0u8; (LAYER_SIZE * LAYER_SIZE * 4) as usize];
    resample_into(&decoded.rgba, decoded.size, &mut out, LAYER_SIZE, [0, 0]);
    Ok(out)
}

/// Layer maps already decoded and resampled to quadrants, by `Map` key. Decoding a map
/// holds its full-size pixels (134 MB for a 4096² map) and stalls the frame, so a
/// rebuild of the atlases for a changed tier or factor reuses the quadrants of every
/// map that is still in use (`build` drops the rest).
#[derive(Default)]
pub(crate) struct Quadrants {
    entries: HashMap<u64, Arc<[u8]>>,
    used: HashSet<u64>,
}

impl Quadrants {
    /// The quadrant for `map`, decoded on first use.
    fn load(&mut self, (key, ktx2): Map<'_>) -> Result<Arc<[u8]>> {
        self.used.insert(key);
        if let Some(quadrant) = self.entries.get(&key) {
            return Ok(Arc::clone(quadrant));
        }
        let quadrant: Arc<[u8]> = quadrant(ktx2)?.into();
        self.entries.insert(key, Arc::clone(&quadrant));
        Ok(quadrant)
    }

    /// Forgets every quadrant no `load` asked for since the last call.
    fn retain_used(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.entries.retain(|key, _| used.contains(key));
    }
}

/// Mean of linear channel `c` of an RGBA8 quadrant, 0 to 1.
fn mean(rgba: &[u8], c: usize) -> f32 {
    let sum: f64 = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| f64::from(p[c]))
        .sum();
    (sum / 255. / (rgba.len() / 4).max(1) as f64) as f32
}

/// Copies `quadrant` (RGBA8, `LAYER_SIZE` square) into atlas cell `i`; with `alpha`,
/// only its alpha channel from channel `from`.
fn place(atlas: &mut [u8], i: usize, quadrant: &[u8], alpha: Option<usize>) {
    let side = 2 * LAYER_SIZE as usize;
    let (x0, y0) = (
        (i & 1) * LAYER_SIZE as usize,
        (i >> 1) * LAYER_SIZE as usize,
    );
    for y in 0..LAYER_SIZE as usize {
        for x in 0..LAYER_SIZE as usize {
            let (src, dst) = (
                (y * LAYER_SIZE as usize + x) * 4,
                ((y0 + y) * side + x0 + x) * 4,
            );
            match alpha {
                Some(from) => atlas[dst + 3] = quadrant[src + from],
                None => atlas[dst..dst + 3].copy_from_slice(&quadrant[src..src + 3]),
            }
        }
    }
}

/// The atlases' pixels and the program's parameters for up to four layers, one splat
/// channel per entry of `layers` (`None`: a layer whose material is not resolved stays on
/// the defaults, so the layers after it keep their channels).
struct Channels {
    atlas: Vec<u8>,
    normals: Vec<u8>,
    any_normal: bool,
    uv: Vec<Value>,
    tint: Vec<Value>,
    packed: [[f32; 4]; 4],
}

/// Fills `Channels` from `layers`. A map that cannot be decoded is reported and left
/// out: its layer keeps the material's factors for it.
fn channels(
    layers: &[Option<LayerMaps<'_>>],
    quadrants: &mut Quadrants,
    gaps: &mut Gaps,
) -> Channels {
    let side = 2 * LAYER_SIZE;
    let mut out = Channels {
        atlas: vec![255u8; (side * side * 4) as usize],
        normals: [128u8, 128, 255, 255].repeat((side * side) as usize),
        any_normal: false,
        uv: vec![Value::Vec4([1., 1., 0., 0.]); 4],
        tint: vec![Value::Vec4([1., 1., 1., 0.5]); 4],
        // Per layer: metal, colour mean luminance, roughness mean, normal scale.
        packed: [[0f32, 0., 1., 0.]; 4],
    };
    let mut load = |map: Map<'_>, gaps: &mut Gaps| match quadrants.load(map) {
        Ok(q) => Some(q),
        Err(error) => {
            gaps.unsupported(
                "terrain layer maps that cannot be decoded",
                &format!("the layer keeps its material's factors for that map ({error:#})"),
            );
            None
        }
    };
    for (i, layer) in layers.iter().take(4).enumerate() {
        let Some(layer) = layer else { continue };
        let m = layer.material;
        if m.base_color_uv.rotation != 0. || m.normal_uv.rotation != 0. {
            gaps.unsupported(
                "rotated terrain layer maps",
                "the layer's maps repeat unrotated",
            );
        }
        if m.base_color_uv.scale.iter().any(|s| *s < 0.) {
            gaps.unsupported(
                "terrain layer maps with a negative scale",
                "the normal map's relief follows the terrain's fixed frame, not the mirrored map",
            );
        }
        if layer.unsampled {
            gaps.unsupported(
                "terrain layer occlusion and emission maps",
                "the layer has no occlusion or glow",
            );
        }
        let t = &m.base_color_uv;
        out.uv[i] = Value::Vec4([t.scale[0], t.scale[1], t.offset[0], t.offset[1]]);
        let [r, g, b, _] = m.base_color;
        out.tint[i] = Value::Vec4([r, g, b, m.roughness.clamp(0.045, 1.)]);
        let [metal, flat, rough, normal] = &mut out.packed[i];
        *metal = m.metallic.clamp(0., 1.);
        if let Some(q) = layer.color.and_then(|map| load(map, gaps)) {
            let [r, g, b] = texture::mean_linear(&q);
            *flat = (0.2126 * r + 0.7152 * g + 0.0722 * b).max(1e-4);
            place(&mut out.atlas, i, &q, None);
        }
        if let Some(q) = layer.metal_roughness.and_then(|map| load(map, gaps)) {
            *rough = mean(&q, 1);
            *metal *= mean(&q, 2);
            // The colour atlas's alpha carries the roughness map where it tiles as the
            // colour map does; otherwise it holds the map's mean, which the layer keeps.
            if m.metallic_roughness_uv == m.base_color_uv {
                place(&mut out.atlas, i, &q, Some(1));
            } else {
                let fill = [0, (*rough * 255.).round() as u8, 0, 0].repeat(q.len() / 4);
                place(&mut out.atlas, i, &fill, Some(1));
                gaps.unsupported(
                    "terrain layer roughness maps tiled unlike their colour maps",
                    "the layer keeps its roughness map's mean",
                );
            }
            if m.metallic > 0. {
                gaps.unsupported(
                    "terrain layer metalness maps up close",
                    "the layer's metalness is its map's mean",
                );
            }
        }
        // A zero normal scale is a flat layer, as no map. The normal map shares the
        // layer's coordinates (Renderlab's 256 parameter bytes hold one set per layer).
        if let Some(map) = layer.normal.filter(|_| m.normal_scale != 0.) {
            if m.normal_uv != m.base_color_uv {
                gaps.unsupported(
                    "terrain layer normal maps tiled unlike their colour maps",
                    "the layer is flat",
                );
            } else if let Some(q) = load(map, gaps) {
                place(&mut out.normals, i, &q, None);
                *normal = m.normal_scale;
                out.any_normal = true;
            }
        }
    }
    quadrants.retain_used();
    out
}

/// Builds the atlases of up to four layers and the parameters that place, tint, roughen
/// and light them, for a detail distance of `detail` metres. A failed upload leaves no
/// texture behind.
pub(crate) fn build(
    engine: &mut Engine,
    layers: &[Option<LayerMaps<'_>>],
    detail: f32,
    quadrants: &mut Quadrants,
    gaps: &mut Gaps,
) -> Result<Splat> {
    let side = 2 * LAYER_SIZE;
    let c = channels(layers, quadrants, gaps);
    let atlas = engine.create_texture(texture::chain(
        [side, side],
        c.atlas,
        TextureRole::BaseColorSrgb,
    ))?;
    let normals = c
        .any_normal
        .then(|| {
            engine.create_texture(texture::chain(
                [side, side],
                c.normals,
                TextureRole::NormalLinear,
            ))
        })
        .transpose();
    let normals = match normals {
        Ok(normals) => normals,
        Err(error) => {
            engine.remove_texture(atlas)?;
            return Err(error.into());
        }
    };
    let detail = detail.max(0.);
    Ok(Splat {
        atlas,
        normals,
        parameters: vec![
            ("uv", Value::Array(c.uv)),
            ("tint", Value::Array(c.tint)),
            ("layer", Value::Array(c.packed.map(Value::Vec4).to_vec())),
            ("detail", Value::Vec4([detail, 0., 0., 0.])),
        ],
    })
}

/// Bilinear resample of `src` (RGBA8, `size`) into the `LAYER_SIZE` square at `at` of
/// `dst` (`side` wide), wrapping at the source's edges as a repeating map does.
fn resample_into(src: &[u8], size: [u32; 2], dst: &mut [u8], side: u32, at: [u32; 2]) {
    let [w, h] = size.map(|s| s.max(1) as usize);
    let texel = |x: usize, y: usize, c: usize| f32::from(src[((y % h) * w + x % w) * 4 + c]);
    for y in 0..LAYER_SIZE {
        for x in 0..LAYER_SIZE {
            let fx = (x as f32 + 0.5) / LAYER_SIZE as f32 * w as f32 - 0.5;
            let fy = (y as f32 + 0.5) / LAYER_SIZE as f32 * h as f32 - 0.5;
            let (x0, y0) = (
                fx.floor().rem_euclid(w as f32) as usize,
                fy.floor().rem_euclid(h as f32) as usize,
            );
            let (ax, ay) = (fx - fx.floor(), fy - fy.floor());
            let out = (((at[1] + y) * side + at[0] + x) * 4) as usize;
            for c in 0..4 {
                let top = texel(x0, y0, c) * (1. - ax) + texel(x0 + 1, y0, c) * ax;
                let bottom = texel(x0, y0 + 1, c) * (1. - ax) + texel(x0 + 1, y0 + 1, c) * ax;
                dst[out + c] = (top * (1. - ay) + bottom * ay).round() as u8;
            }
        }
    }
}

/// The program, composed on first use; None (reported once) when Renderlab rejects it.
pub(crate) fn program(
    slot: &mut Option<Option<ProgramId>>,
    engine: &mut Engine,
    gaps: &mut Gaps,
) -> Option<ProgramId> {
    if slot.is_none() {
        let created = source()
            .ok_or_else(|| "its standard program changed shape".to_owned())
            .and_then(|source| {
                engine
                    .create_program("ara-terrain-splat", &source)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = &created {
            gaps.unsupported(
                "terrain layer maps",
                &format!("each layer shows its base colour factor ({error})"),
            );
        }
        *slot = Some(created.ok());
    }
    slot.flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_splat_program_replaces_the_standard_surface() {
        let source = source().expect("the standard program ends as expected");
        assert_eq!(source.matches("fn surface(").count(), 1);
        assert!(source.contains("fn ara_atlas(") && source.contains("fn shade("));
    }

    fn material(colour: [f32; 3], roughness: f32) -> ModelMaterial {
        ModelMaterial {
            base_color: [colour[0], colour[1], colour[2], 1.],
            roughness,
            ..Default::default()
        }
    }

    fn layer(material: &ModelMaterial) -> LayerMaps<'_> {
        LayerMaps {
            material,
            color: None,
            normal: None,
            metal_roughness: None,
            unsampled: false,
        }
    }

    /// A layer whose material is not resolved stays on the defaults and the layers after
    /// it keep their own channels.
    #[test]
    fn an_unresolved_layer_does_not_shift_the_next_ones() {
        let (b, c) = (
            material([0.2, 0.4, 0.6], 0.3),
            material([0.7, 0.1, 0.1], 0.8),
        );
        let layers = [None, Some(layer(&b)), Some(layer(&c))];
        let out = channels(&layers, &mut Quadrants::default(), &mut Gaps::default());
        assert_eq!(out.tint[0], Value::Vec4([1., 1., 1., 0.5]));
        assert_eq!(out.tint[1], Value::Vec4([0.2, 0.4, 0.6, 0.3]));
        assert_eq!(out.tint[2], Value::Vec4([0.7, 0.1, 0.1, 0.8]));
    }

    /// A map that cannot be decoded is reported and the layer keeps its factors; the
    /// rest of the terrain still builds.
    #[test]
    fn an_undecodable_map_costs_that_map_only() {
        let (a, b) = (
            material([0.5, 0.5, 0.5], 0.4),
            material([0.1, 0.2, 0.3], 0.6),
        );
        let mut broken = layer(&a);
        broken.color = Some((7, b"not a ktx2 file"));
        let mut gaps = Gaps::default();
        let out = channels(
            &[Some(broken), Some(layer(&b))],
            &mut Quadrants::default(),
            &mut gaps,
        );
        assert!(gaps.reported("terrain layer maps that cannot be decoded"));
        assert_eq!(out.tint[0], Value::Vec4([0.5, 0.5, 0.5, 0.4]));
        assert_eq!(out.tint[1], Value::Vec4([0.1, 0.2, 0.3, 0.6]));
    }

    /// A rebuild takes a map's quadrant from the cache instead of decoding it again (the
    /// bytes below would not decode), and forgets quadrants no layer uses any more.
    #[test]
    fn rebuilding_reuses_decoded_quadrants_and_drops_unused_ones() {
        let a = material([1.; 3], 0.5);
        let mut quadrants = Quadrants::default();
        let pixel = [10u8, 20, 30, 255];
        let solid: Arc<[u8]> = pixel.repeat((LAYER_SIZE * LAYER_SIZE) as usize).into();
        quadrants.entries.insert(1, Arc::clone(&solid));
        quadrants.entries.insert(2, solid);
        let mut cached = layer(&a);
        cached.color = Some((1, b"never decoded"));
        let mut gaps = Gaps::default();
        let out = channels(&[Some(cached)], &mut quadrants, &mut gaps);
        assert!(!gaps.reported("terrain layer maps that cannot be decoded"));
        assert_eq!(&out.atlas[..4], &pixel);
        assert!(quadrants.entries.contains_key(&1) && !quadrants.entries.contains_key(&2));
    }

    #[test]
    fn resampling_fills_its_quadrant_only() {
        let src = [10u8, 20, 30, 40].repeat(4);
        let side = 2 * LAYER_SIZE;
        let mut atlas = vec![0u8; (side * side * 4) as usize];
        resample_into(&src, [2, 2], &mut atlas, side, [LAYER_SIZE, 0]);
        assert_eq!(
            &atlas[(LAYER_SIZE * 4) as usize..(LAYER_SIZE * 4 + 4) as usize],
            &[10, 20, 30, 40]
        );
        assert_eq!(&atlas[..4], &[0; 4]);
    }

    /// Renderlab rejects a program whose parameters pass 256 bytes, which would leave
    /// every tile on its layers' colour factors: count the struct's members.
    #[test]
    fn the_parameters_fit_renderlabs_256_bytes() {
        let start = SURFACE.find("struct Parameters {").expect("parameters");
        let members = &SURFACE[start..start + SURFACE[start..].find("};").expect("end")];
        let arrays = members.matches("array<vec4<f32>,4>").count();
        let vectors = members.matches("vec4<f32>").count() - arrays;
        assert!(
            arrays * 64 + vectors * 16 <= 256,
            "{arrays} arrays, {vectors} vectors"
        );
    }

    /// A roughness map goes to its cell's alpha only, leaving the colour there.
    #[test]
    fn roughness_fills_its_cells_alpha_only() {
        let side = 2 * LAYER_SIZE as usize;
        let mut atlas = vec![7u8; side * side * 4];
        let q = [1u8, 2, 3, 4].repeat((LAYER_SIZE * LAYER_SIZE) as usize);
        place(&mut atlas, 3, &q, Some(1));
        let at = ((side - 1) * side + side - 1) * 4;
        assert_eq!(&atlas[at..at + 4], &[7, 7, 7, 2]);
        assert_eq!(&atlas[..4], &[7; 4]);
    }
}
