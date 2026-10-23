//! The runtime reads material extension data from real cooked artifacts.
use ara_asset::{
    CachedImport, Cancellation, CookPreset, ImportContext, ImportInput, TargetPlatform,
};
use ara_runtime::model::{Model, TextureTransform, TextureUsage};
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn cooked_material_extensions_keep_independent_uvs_emission_transmission_and_coverage() {
    let registry = ara_importers::registry().expect("registry");
    let importer = registry.get("ara.gltf").expect("installed importer");
    let input = ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 4,
        source_path: "panel.gltf".into(),
        source: include_bytes!("../../ara-importers/tests/fixtures/material-extensions.gltf")
            .to_vec(),
        importer: importer.id().into(),
        version: importer.version().into(),
        settings: json!({"virtualGeometry":false,"opacityMaps":{"#0":"opacity.png"}}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::from([(
            "opacity.png".into(),
            include_bytes!("../../ara-importers/tests/fixtures/companion-opacity.png").to_vec(),
        )]),
        asset_dependencies: BTreeMap::new(),
    };
    let output = registry
        .import(
            &input,
            &mut ImportContext {
                metrics: None,
                cancellation: &Cancellation::default(),
                progress: &mut |_| {},
            },
        )
        .expect("cook");
    let digest = input.key().expect("key");
    let model = Model::decode(
        "as_panel",
        digest,
        &CachedImport {
            key: digest,
            output,
        },
    )
    .expect("model");
    let material = &model.materials[0];
    assert_eq!(material.name, "Layered");
    assert_eq!(material.base_color_uv.tex_coord, 1);
    assert_eq!(material.normal_uv.tex_coord, 1);
    assert_ne!(material.base_color_uv, material.normal_uv);
    assert_ne!(material.base_color_uv, material.emissive_uv);
    assert_ne!(material.occlusion_uv, material.transmission_uv);
    let uv = material.base_color_uv.apply([1., 0.]);
    assert!(
        (uv[0] - 0.25).abs() < 1e-5 && (uv[1] - 3.5).abs() < 1e-5,
        "{uv:?}"
    );
    for (actual, expected) in material.emissive.into_iter().zip([0.5, 1., 1.5]) {
        assert!((actual - expected).abs() < 1e-6);
    }
    assert_eq!(material.transmission_factor, 0.75);
    let transmission = material.transmission_texture.expect("transmission texture");
    assert_eq!(model.textures[transmission].usage, TextureUsage::Linear);
    assert_eq!(material.occlusion_texture, Some(transmission));
    assert_eq!(material.occlusion_strength, 0.6);
    assert_eq!(
        material.alpha_mode, 1,
        "transmission does not replace alpha coverage"
    );
    assert!(model.primitives[0].vertices.iter().any(|v| v.uv != v.uv1));
    // UV1=(world Z, world X). Invert its rotated/scaled normal coordinates
    // analytically to obtain the expected +U surface tangent.
    let x = 0.3f32.sin() / 0.75;
    let z = 0.3f32.cos() / 0.5;
    let length = (x * x + z * z).sqrt();
    assert!(
        model.primitives[0]
            .vertices
            .iter()
            .all(|v| { (v.tangent[0] * x + v.tangent[2] * z) / length > 0.999 }),
        "normal tangents must use the selected/transformed UV1, not UV0"
    );
    assert_eq!(
        ara_runtime::model::ModelMaterial::default().transmission_uv,
        TextureTransform::default()
    );
}
