//! Material AST adoption: generated evaluator is embedded into all terrain passes.
use anyhow::{Result, anyhow};

/// Surface definition: weight-map blend with triplanar cliff textures.
pub const TERRAIN_DSL: &str = r#"material Landscape : surface budget 1 {
  param grass : TextureColor = @textures/grass.png#tx_grass001;
  param rock : TextureColor = @textures/rock.png#tx_rock0001;
  param soil : TextureColor = @textures/soil.png#tx_soil0001;
  param snow : TextureColor = @textures/snow.png#tx_snow0001;
  param scale : float = 0.12;
  custom layerWeights(position: vec3) -> vec4 cost(alu: 20) doc "Normalized terrain splat weights." slang "Frame f=frame(); uint w=sampleData(f,int2(position.xz/f.params.x)).y; return float4(w&255,(w>>8)&255,(w>>16)&255,(w>>24)&255)/255.;";
  custom splat(a: ColorLinear, b: ColorLinear, c: ColorLinear, d: ColorLinear, weights: vec4) -> ColorLinear cost(alu: 24) doc "Four normalized material channels." slang "return a*weights.x+b*weights.y+c*weights.z+d*weights.w;";
  let position : vec3 = worldPosition();
  let normal : vec3 = worldNormal();
  let grassColor : ColorLinear = triplanar(grass, position, normal, scale);
  let rockColor : ColorLinear = triplanar(rock, position, normal, scale);
  let soilColor : ColorLinear = triplanar(soil, position, normal, scale);
  let snowColor : ColorLinear = triplanar(snow, position, normal, scale);
  let weights : vec4 = layerWeights(position);
  let color : ColorLinear = splat(grassColor, rockColor, soilColor, snowColor, weights);
  let roughness : float = 0.88;
  output baseColor = color;
  output roughness = roughness;
}"#;

/// Produces a checked material AST and generated evaluator for offline cooking.
/// The landscape adapter supplies bindless helpers for ara-render's larger heap.
pub fn evaluator() -> Result<String> {
    let m = ara_material::dsl::parse(TERRAIN_DSL, "gr_land0001").map_err(|d| anyhow!("{d:?}"))?;
    let g = ara_material::generate(&m, &ara_material::Instance::default())
        .map_err(|d| anyhow!("{d:?}"))?;
    let start = ara_material::ENGINE_TEMPLATE.len();
    let end = g
        .source
        .find("// Offline verification entry.")
        .ok_or_else(|| anyhow!("generated evaluator boundary missing"))?;
    Ok(g.source[start..end].to_owned())
}
