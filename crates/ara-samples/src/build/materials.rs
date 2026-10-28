//! Native surface graphs bind imported texture assets, never derived glTF materials.
use super::{
    Ctx, bounds,
    place::{Models, short},
};
use anyhow::{Context, Result, ensure};
use serde_json::json;

fn graph(ctx: &Ctx, name: &str, text: String) -> Result<String> {
    let path = format!("materials/{name}.graph");
    let id = format!("gr_{}", short(name));
    ctx.new_document(&path, "material", &id)?;
    ctx.apply(
        &path,
        "Author a native PBR surface",
        vec![("material.text.apply", json!({"text":text}))],
    )?;
    Ok(format!("@{path}#{id}"))
}

pub(super) fn palette(
    ctx: &Ctx,
    models: &mut Models,
    name: &str,
    asset: &str,
    palette: &str,
) -> Result<()> {
    super::native::whole(ctx, models, name, asset, &[])?;
    let (_, source) = super::native::source(ctx, asset)?;
    let texture_path = format!("kit/{palette}");
    let texture_id = format!("as_{}", short(&format!("{name}palette")));
    ctx.sidecar(&texture_path, &texture_id, json!({"usage":"color"}))?;
    let texture = format!("@{texture_path}#{texture_id}");
    let mut graphs = std::collections::BTreeMap::<usize, String>::new();
    let mut overrides = Vec::new();
    for (slot, material) in bounds::material_slots(&source)?.into_iter().enumerate() {
        let source_material = &source["materials"][material];
        let pbr = &source_material["pbrMetallicRoughness"];
        let Some(index) = pbr["baseColorTexture"]["index"].as_u64() else {
            continue;
        };
        let image = source["textures"][index as usize]["source"]
            .as_u64()
            .context("palette texture has no image")? as usize;
        if !source["images"][image]["name"]
            .as_str()
            .is_some_and(|name| name.contains("color-palette"))
        {
            continue;
        }
        let reference = if let Some(reference) = graphs.get(&material) {
            reference.clone()
        } else {
            ensure!(
                source_material["normalTexture"].is_null()
                    && pbr["metallicRoughnessTexture"].is_null(),
                "palette source unexpectedly has extra texture slots"
            );
            let rgb: [f64; 3] =
                std::array::from_fn(|i| pbr["baseColorFactor"][i].as_f64().unwrap_or(1.));
            let metallic = pbr["metallicFactor"].as_f64().unwrap_or(1.);
            let roughness = pbr["roughnessFactor"].as_f64().unwrap_or(1.);
            let text = format!(
                "material Suit : surface budget 1 {{ param palette : TextureColor = {texture}; let coords : vec2 = uv(); let sample : ColorLinear = sampleColor(palette, coords); let factor : ColorLinear = linear({}, {}, {}); let color : ColorLinear = mul(sample, factor); let roughness : float = {roughness}; let metallic : float = {metallic}; output baseColor = color; output roughness = roughness; output metallic = metallic; }}",
                rgb[0], rgb[1], rgb[2]
            );
            let reference = graph(ctx, &format!("{name}palette{material}"), text)?;
            graphs.insert(material, reference.clone());
            reference
        };
        overrides.push(json!({"primitive":slot,"material":reference}));
    }
    ensure!(!overrides.is_empty(), "no palette-textured primitive found");
    models
        .0
        .get_mut(name)
        .context("palette model was not registered")?
        .renderer["materialOverrides"] = json!(overrides);
    Ok(())
}

pub(super) fn terrain(ctx: &Ctx, name: &str, set: &str, tint: [f32; 3]) -> Result<String> {
    let entry = ctx
        .lock
        .get(set)
        .with_context(|| format!("{set} is not pinned"))?;
    let meters = entry.meters.unwrap_or([1.; 2]);
    let mut maps = Vec::new();
    for (role, suffix, usage) in [
        ("color", "_diff_", "color"),
        ("normal", "_nor_gl_", "normal"),
        ("arm", "_arm_", "linear"),
    ] {
        let file = entry
            .files
            .iter()
            .find(|file| {
                file.path.contains(suffix)
                    && [".png", ".jpg", ".jpeg"]
                        .iter()
                        .any(|extension| file.path.ends_with(extension))
            })
            .with_context(|| format!("{set} has no {role} map"))?;
        let path = entry.relative(&file.path);
        let id = format!("as_{}", short(&format!("{name}{role}")));
        ctx.sidecar(&path, &id, json!({"usage":usage}))?;
        maps.push(format!("@{path}#{id}"));
    }
    let text = format!(
        "material Ground : surface budget 1 {{ param colorMap : TextureColor = {}; param normalMap : TextureNormal = {}; param armMap : TextureData = {}; let world : vec2 = uv(); let repeat : vec2 = vec2({}, {}); let coords : vec2 = mul(world, repeat); let sample : ColorLinear = sampleColor(colorMap, coords); let tint : ColorLinear = linear({}, {}, {}); let color : ColorLinear = mul(sample, tint); let normal : vec3 = sampleNormal(normalMap, coords); let arm : vec4 = sampleData(armMap, coords); let green : vec4 = vec4(0, 1, 0, 0); let blue : vec4 = vec4(0, 0, 1, 0); let roughness : float = dot(arm, green); let metallic : float = dot(arm, blue); output baseColor = color; output normal = normal; output roughness = roughness; output metallic = metallic; }}",
        maps[0],
        maps[1],
        maps[2],
        1. / meters[0],
        1. / meters[1],
        tint[0],
        tint[1],
        tint[2]
    );
    graph(ctx, name, text)
}
