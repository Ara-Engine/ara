//! Material extension preservation and companion images in deterministic cooks.
use ara_asset::{
    Cancellation, CookPreset, ImportContext, ImportInput, ImportOutput, TargetPlatform,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SOURCE: &[u8] = include_bytes!("fixtures/material-extensions.gltf");
const MASK: &[u8] = include_bytes!("fixtures/companion-opacity.png");

fn input(source: Vec<u8>, settings: Value) -> ImportInput {
    let registry = ara_importers::registry().expect("registry");
    let importer = registry.get("ara.gltf").expect("installed importer");
    ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 4,
        source_path: "panel.gltf".into(),
        source,
        importer: importer.id().into(),
        version: importer.version().into(),
        settings,
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::from([("opacity.png".into(), MASK.to_vec())]),
        asset_dependencies: BTreeMap::new(),
    }
}

fn cook(input: &ImportInput) -> ara_asset::Result<ImportOutput> {
    ara_importers::registry()?.import(
        input,
        &mut ImportContext {
            metrics: None,
            cancellation: &Cancellation::default(),
            progress: &mut |_| {},
        },
    )
}

fn manifest(output: &ImportOutput) -> Value {
    serde_json::from_slice(
        &output
            .artifacts
            .iter()
            .find(|a| a.name == "scene.json")
            .expect("manifest")
            .bytes,
    )
    .expect("JSON")
}

#[test]
fn companion_opacity_cooks_into_alpha_and_all_material_extensions_survive() {
    let registry = ara_importers::registry().expect("registry");
    let settings = json!({"virtualGeometry":false,"opacityMaps":{"#0":"opacity.png"}});
    let deps = registry
        .get("ara.gltf")
        .expect("importer")
        .dependencies_with_settings(SOURCE, &settings)
        .expect("discovery");
    assert_eq!(deps, ["opacity.png"]);
    let mut input = input(SOURCE.to_vec(), settings);
    let output = cook(&input).expect("cook");
    input.thread_budget = 1;
    assert_eq!(cook(&input).expect("single thread"), output);
    let manifest = manifest(&output);
    let material = &manifest["document"]["materials"][0];
    assert_eq!(material["alphaMode"], "MASK");
    assert_eq!(
        material["normalTexture"]["extensions"]["KHR_texture_transform"]["texCoord"],
        1
    );
    assert_eq!(
        material["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"],
        5
    );
    assert_eq!(
        material["extensions"]["KHR_materials_transmission"]["transmissionFactor"],
        0.75
    );
    let texture = output
        .artifacts
        .iter()
        .find(|a| a.name == "textures/0-Color.ktx2")
        .expect("color texture");
    let texture = ara_asset_format::Texture::read(&texture.bytes).expect("KTX2");
    let mut pixels = [0u32; 64];
    texture2ddecoder::decode_bc7(
        texture.levels().next().expect("base mip"),
        8,
        8,
        &mut pixels,
    )
    .expect("BC7");
    for (i, pixel) in pixels.iter().enumerate() {
        let alpha = (pixel >> 24) as i32;
        let expected = if i % 8 < 4 { 0 } else { 128 };
        assert!((alpha - expected).abs() <= 4, "pixel {i}: alpha {alpha}");
        assert!(
            ((pixel >> 16 & 255) as i32 - 180).abs() <= 12,
            "color changed: {pixel:x}"
        );
    }
}

#[test]
fn transmission_only_images_are_cooked_and_opacity_preserves_authored_blending() {
    let mut source: Value = serde_json::from_slice(SOURCE).expect("fixture");
    source["materials"][0]["pbrMetallicRoughness"]
        .as_object_mut()
        .expect("PBR")
        .remove("metallicRoughnessTexture");
    source["materials"][0]
        .as_object_mut()
        .expect("material")
        .remove("occlusionTexture");
    source["materials"][0]["alphaMode"] = "BLEND".into();
    let bytes = ara_json::to_canonical_string(&ara_json::sort_keys(&source)).into_bytes();
    let request = input(
        bytes,
        json!({"virtualGeometry":false,"opacityMaps":{"#0":"opacity.png"}}),
    );
    let output = cook(&request).expect("cook");
    let manifest = manifest(&output);
    assert_eq!(manifest["document"]["materials"][0]["alphaMode"], "BLEND");
    let texture = output
        .artifacts
        .iter()
        .find(|a| a.name == "textures/1-Linear.ktx2")
        .expect("transmission-only image");
    let texture = ara_asset_format::Texture::read(&texture.bytes).expect("KTX2");
    let mut pixels = [0u32; 64];
    texture2ddecoder::decode_bc7(
        texture.levels().next().expect("base mip"),
        8,
        8,
        &mut pixels,
    )
    .expect("BC7");
    assert!(
        pixels
            .iter()
            .all(|p| ((p >> 16 & 255) as i32 - 64).abs() <= 4)
    );
    let mut bad = request;
    bad.settings["opacityMaps"] = json!({"#999":"opacity.png"});
    assert!(cook(&bad).is_err());
}

#[test]
fn opacity_map_names_resolve_external_image_uris_and_preserve_original_input_bytes() {
    use base64::Engine;
    let mut source: Value = serde_json::from_slice(SOURCE).expect("fixture");
    let data = source["images"][0]["uri"]
        .as_str()
        .expect("URI")
        .strip_prefix("data:image/png;base64,")
        .expect("PNG data");
    let color = base64::engine::general_purpose::STANDARD
        .decode(data)
        .expect("base64");
    source["images"][0]["uri"] = "textures/leaf%20color.png".into();
    let settings =
        json!({"virtualGeometry":false,"opacityMaps":{"textures/leaf color.png":"opacity.png"}});
    let registry = ara_importers::registry().expect("registry");
    let bytes = ara_json::to_canonical_string(&ara_json::sort_keys(&source)).into_bytes();
    let deps = registry
        .get("ara.gltf")
        .expect("importer")
        .dependencies_with_settings(&bytes, &settings)
        .expect("dependencies");
    assert_eq!(deps, ["opacity.png", "textures/leaf color.png"]);
    let mut request = input(bytes, settings);
    request
        .dependencies
        .insert("textures/leaf color.png".into(), color.clone());
    let before = request.key().expect("snapshot key");
    let output = cook(&request).expect("URI opacity cook");
    assert_eq!(
        manifest(&output)["document"]["materials"][0]["alphaMode"],
        "MASK"
    );
    assert_eq!(request.dependencies["textures/leaf color.png"], color);
    assert_eq!(request.key().expect("unchanged input"), before);
}
