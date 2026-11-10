//! Terrain layer maps on Renderlab: up to four layers' base-colour maps in one 2x2 atlas
//! shared by every tile, each tile's splat weights in its metallic-roughness slot, and a
//! program (Renderlab's standard program with its `surface` replaced) that blends the
//! layers. A layer's map repeats over the ground in world metres by its base-colour uv
//! transform, as Ara's layer graphs sample `uv(world) * repeat`.
use crate::{gaps::Gaps, texture};
use anyhow::Result;
use ara_runtime::model::ModelMaterial;
use renderlab::{
    asset::TextureRole,
    engine::{Engine, ProgramId, TextureId, Value},
};

/// Side of one layer's quadrant in the atlas, texels.
const LAYER_SIZE: u32 = 512;

/// The splat program's `struct Parameters` and the `surface` that reads it.
const SURFACE: &str = r#"
struct Parameters { uv:array<vec4<f32>,4>, tint:array<vec4<f32>,4>, metal:vec4<f32> };
const DEFAULT_PARAMETERS:Parameters=Parameters(
    array<vec4<f32>,4>(vec4<f32>(1.0,1.0,0.0,0.0),vec4<f32>(1.0,1.0,0.0,0.0),vec4<f32>(1.0,1.0,0.0,0.0),vec4<f32>(1.0,1.0,0.0,0.0)),
    array<vec4<f32>,4>(vec4<f32>(1.0,1.0,1.0,0.5),vec4<f32>(1.0,1.0,1.0,0.5),vec4<f32>(1.0,1.0,1.0,0.5),vec4<f32>(1.0,1.0,1.0,0.5)),
    vec4<f32>(0.0));
// Layer i's base colour at this receiver: its map repeats in world metres (uv xy the
// repeat per metre, zw the offset) inside quadrant i of the 2x2 atlas, kept half a texel
// of the sampled level inside it so the quadrant's neighbours never bleed in.
fn ara_layer(input:SurfaceInput, i:u32) -> vec3<f32> {
    let t=params.uv[i];
    let uv=input.world.xz*t.xy+t.zw;
    let dx=input.world_dx.xz*t.xy*0.5;
    let dy=input.world_dy.xz*t.xy*0.5;
    let texels=max(length(dx),length(dy))*ATLAS_SIZE;
    let margin=clamp(0.5*max(texels,1.0)/(0.5*ATLAS_SIZE),0.0,0.25);
    let cell=vec2<f32>(f32(i&1u),f32(i>>1u));
    let at=(cell+clamp(fract(uv),vec2<f32>(margin),vec2<f32>(1.0-margin)))*0.5;
    return sample_base_color_grad(input,at,dx,dy).rgb*params.tint[i].rgb;
}
fn surface(input:SurfaceInput) -> Surface {
    let raw=sample_metallic_roughness_grad(input,input.uv,input.uv_dx,input.uv_dy);
    let w=raw/max(raw.x+raw.y+raw.z+raw.w,1e-4);
    var base=vec3<f32>(0.0);
    for(var i=0u;i<4u;i+=1u) { if w[i]>0.0 { base+=w[i]*ara_layer(input,i); } }
    let rough=clamp(dot(w,vec4<f32>(params.tint[0].a,params.tint[1].a,params.tint[2].a,params.tint[3].a)),0.045,1.0);
    let metal=clamp(dot(w,params.metal),0.0,1.0);
    let n=material_normal(input);
    let decal_base=decal_base_color(input,vec4<f32>(clamp(base,vec3<f32>(0.0),vec3<f32>(1.0)),1.0));
    let decal_mr=decal_metallic_roughness(input,vec2<f32>(rough,metal));
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
            "const ATLAS_SIZE:f32={}.0;\n{}{SURFACE}{}",
            2 * LAYER_SIZE,
            &standard[..start],
            &standard[end..]
        )
    })
}

/// The resolved layers of a terrain: the atlas and the program's parameters.
pub(crate) struct Splat {
    pub atlas: TextureId,
    pub parameters: [(&'static str, Value); 3],
}

/// Builds the atlas of up to four layer materials (their base-colour maps; white for a
/// layer without one) and the parameters that place, tint and roughen them.
pub(crate) fn build(
    engine: &mut Engine,
    layers: &[(&ModelMaterial, Option<&[u8]>)],
    gaps: &mut Gaps,
) -> Result<Splat> {
    let side = 2 * LAYER_SIZE;
    let mut atlas = vec![255u8; (side * side * 4) as usize];
    let mut uv = vec![Value::Vec4([1., 1., 0., 0.]); 4];
    let mut tint = vec![Value::Vec4([1., 1., 1., 0.5]); 4];
    let mut metal = [0f32; 4];
    for (i, (material, ktx2)) in layers.iter().take(4).enumerate() {
        let t = &material.base_color_uv;
        if t.rotation != 0. {
            gaps.unsupported(
                "rotated terrain layer maps",
                "the layer's map repeats unrotated",
            );
        }
        uv[i] = Value::Vec4([t.scale[0], t.scale[1], t.offset[0], t.offset[1]]);
        let [r, g, b, _] = material.base_color;
        tint[i] = Value::Vec4([r, g, b, material.roughness.clamp(0.045, 1.)]);
        metal[i] = material.metallic.clamp(0., 1.);
        if let Some(ktx2) = ktx2 {
            let decoded = ara_runtime_render::decode_texture(ktx2)?;
            let (x0, y0) = ((i as u32 & 1) * LAYER_SIZE, (i as u32 >> 1) * LAYER_SIZE);
            resample_into(&decoded.rgba, decoded.size, &mut atlas, side, [x0, y0]);
        }
    }
    let atlas = engine.create_texture(texture::chain(
        [side, side],
        atlas,
        TextureRole::BaseColorSrgb,
    ))?;
    Ok(Splat {
        atlas,
        parameters: [
            ("uv", Value::Array(uv)),
            ("tint", Value::Array(tint)),
            ("metal", Value::Vec4(metal)),
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
        assert!(source.contains("fn ara_layer(") && source.contains("fn shade("));
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
}
