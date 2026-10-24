#![allow(clippy::unwrap_used)]
use super::*;
use ara_asset::{Artifact, ArtifactKind, CachedImport, ImportOutput};
use ara_asset_format::{TextureFormat, write_ktx2};
use ara_material::{InstanceAsset, dsl};

fn fixture(text: &str) -> (Workspace, Material) {
    let material = dsl::parse(text, "gr_test").unwrap();
    let mut workspace = Workspace::default();
    workspace.insert(
        "test.graph",
        material
            .to_graph(Default::default())
            .unwrap()
            .to_document()
            .unwrap(),
    );
    (workspace, material)
}

#[test]
fn instances_specialize_parameters_and_failed_reload_retains_last_good() {
    let (mut workspace, definition) = fixture(
        "material Test : surface budget 1 { param tint : ColorLinear = linear(1, 0, 0); param rough : float = 0.4; output baseColor = tint; output roughness = rough; }",
    );
    let instance = InstanceAsset {
        definition: ara_schema::AssetRef("@test.graph#gr_test".into()),
        overrides: [(
            definition.node_id("tint").unwrap().to_owned(),
            ParameterValue::Numeric(vec![0., 1., 0.]),
        )]
        .into(),
    };
    workspace.insert(
        "test.materialinstance",
        instance
            .to_document(ara_doc::AssetId::parse("mi_test").unwrap())
            .unwrap(),
    );
    let mut materials = Materials::default();
    let source = crate::assets::MemoryAssets::default();
    assert!(materials.load(&workspace, &source, "mi_test").unwrap());
    let first = Arc::clone(materials.get("mi_test").unwrap());
    assert_eq!(first.material.base_color, [0., 1., 0., 1.]);
    assert_eq!(first.material.roughness, 0.4);
    assert!(!materials.load(&workspace, &source, "mi_test").unwrap());
    let mut invalid = instance;
    invalid
        .overrides
        .insert("n_missing".into(), ParameterValue::Numeric(vec![0.]));
    workspace.insert(
        "test.materialinstance",
        invalid
            .to_document(ara_doc::AssetId::parse("mi_test").unwrap())
            .unwrap(),
    );
    assert!(materials.load(&workspace, &source, "mi_test").is_err());
    assert!(Arc::ptr_eq(&first, materials.get("mi_test").unwrap()));
}

#[test]
fn shared_samples_preserve_uv_channel_and_color_sampling_policy() {
    let (mut workspace, _) = fixture(
        "material Test : surface budget 1 { param image : TextureColor = @image.png#as_image; param raw : TextureData = @image.png#as_image; let uv0 : vec2 = uv(); let scale : vec2 = vec2(2, 3); let coords : vec2 = mul(uv0, scale); let base : ColorLinear = sampleColor(image, coords); let data : vec4 = sampleData(raw, coords); let mask : float = alpha(data); let green : vec4 = vec4(0, 1, 0, 0); let blue : vec4 = vec4(0, 0, 1, 0); let rough : float = dot(data, green); let metal : float = dot(data, blue); output baseColor = base; output emissive = base; output opacityMask = mask; output roughness = rough; output metallic = metal; }",
    );
    let mut index = workspace.index().clone();
    index.insert("as_image", "image.png", Some("import".into()));
    let mut indexed = Workspace::new(index);
    for (path, doc) in workspace.docs() {
        indexed.insert(path, doc.clone());
    }
    workspace = indexed;
    let key = Digest::of(b"first");
    let mut source = crate::assets::MemoryAssets(
        [(
            "as_image".into(),
            CachedImport {
                key,
                output: ImportOutput {
                    artifacts: vec![Artifact {
                        name: "texture.ktx2".into(),
                        kind: ArtifactKind::Texture,
                        bytes: write_ktx2(4, 4, TextureFormat::Bc7Srgb, &[vec![0; 16]]).unwrap(),
                    }],
                },
            },
        )]
        .into(),
    );
    let first = MaterialAsset::compile(&workspace, &source, "gr_test").unwrap();
    assert_eq!(first.textures.len(), 1);
    assert_eq!(
        first.texture_generations,
        vec![TextureGeneration {
            asset: "as_image".into(),
            digest: key
        }]
    );
    assert!(first.base_color_decode_srgb && first.emissive_decode_srgb);
    assert_eq!(first.material.base_color_uv.scale, [2., 3.]);
    assert_eq!(
        first.material.base_color_texture,
        first.material.metallic_roughness_texture
    );
    assert_eq!(first.material.alpha_mode, 1);
    source.0.get_mut("as_image").unwrap().key = Digest::of(b"second");
    let second = MaterialAsset::compile(&workspace, &source, "gr_test").unwrap();
    assert_ne!(first.digest, second.digest);
}

#[test]
fn unsupported_reachable_surface_expressions_diagnose_with_node_identity() {
    let (workspace, material) = fixture(
        "material Test : surface budget 1 { let coat : float = 0.5; output clearCoat = coat; }",
    );
    let error = MaterialAsset::compile(
        &workspace,
        &crate::assets::MemoryAssets::default(),
        "gr_test",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains(&format!("gr_test#{}", material.node_id("coat").unwrap())),
        "{error}"
    );
    assert!(error.contains("clear coat"), "{error}");
}
