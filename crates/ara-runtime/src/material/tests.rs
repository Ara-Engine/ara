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

fn compile(text: &str) -> Result<MaterialAsset> {
    let (workspace, _) = fixture(text);
    MaterialAsset::compile(
        &workspace,
        &crate::assets::MemoryAssets::default(),
        "gr_test",
    )
}

/// Three colours a, b, c, the vertex colour `vc` and its channels r, g, b and alpha.
const PALETTE: &str = "let vc : vec4 = vertexColor(); let r : float = red(vc); let g : float = green(vc);
    let bl : float = blue(vc); let al : float = alpha(vc);
    let a : ColorLinear = linear(1, 0, 0); let b : ColorLinear = linear(0, 1, 0); let c : ColorLinear = linear(0, 0, 1);";

fn graph(body: &str) -> Result<MaterialAsset> {
    compile(&format!(
        "material T : surface budget 1 {{ {PALETTE} {body} }}"
    ))
}

#[test]
fn vertex_colour_lerps_flatten_into_paint_layers() {
    let asset = graph(
        "let ab : ColorLinear = lerp(a, b, r); let abc : ColorLinear = lerp(ab, c, g);
         let rough : float = 0.3; output baseColor = abc; output roughness = rough;",
    )
    .unwrap();
    let layers: Vec<_> = asset
        .paint_layers
        .iter()
        .map(|l| (l.terms, l.material.base_color, l.material.roughness))
        .collect();
    // lerp(lerp(a, b, r), c, g): a (1-r)(1-g), b r(1-g), c g.
    assert_eq!(
        layers,
        [
            ([2, 2, 0, 0], [1., 0., 0., 1.], 0.3),
            ([1, 2, 0, 0], [0., 1., 0., 1.], 0.3),
            ([0, 1, 0, 0], [0., 0., 1., 1.], 0.3),
        ]
    );
    assert!(!asset.material.vertex_color_tint);
}

#[test]
fn outputs_lerped_by_different_channels_refine_into_shared_layers() {
    let asset = graph(
        "let base : ColorLinear = lerp(a, b, r);
         let one : float = 1; let inverse : float = sub(one, al);
         let e0 : ColorLinear = linear(0, 0, 0); let e1 : ColorLinear = linear(2, 2, 2);
         let glow : ColorLinear = lerp(e0, e1, inverse);
         output baseColor = base; output emissive = glow;",
    )
    .unwrap();
    let mut layers: Vec<_> = asset
        .paint_layers
        .iter()
        .map(|l| (l.terms, l.material.base_color[0], l.material.emissive[0]))
        .collect();
    layers.sort_by_key(|l| l.0);
    // Every (red, alpha) side combination; `1 - a` swaps alpha's sides.
    assert_eq!(
        layers,
        [
            ([1, 0, 0, 1], 0., 0.),
            ([1, 0, 0, 2], 0., 2.),
            ([2, 0, 0, 1], 1., 0.),
            ([2, 0, 0, 2], 1., 2.),
        ]
    );
}

#[test]
fn base_colour_times_vertex_colour_is_the_tint() {
    let asset = graph(
        "let rgbv : vec3 = rgb(vc); let tint : ColorLinear = color(rgbv);
         let grey : ColorLinear = linear(0.5, 0.5, 0.5); let base : ColorLinear = mul(grey, tint);
         output baseColor = base;",
    )
    .unwrap();
    assert!(asset.material.vertex_color_tint && asset.paint_layers.is_empty());
    assert_eq!(asset.material.base_color, [0.5, 0.5, 0.5, 1.]);
}

#[test]
fn unrepresentable_vertex_colour_graphs_diagnose() {
    for (body, expected) in [
        (
            "let half : float = 0.5; let w : float = mul(r, half); let ab : ColorLinear = lerp(a, b, w); output baseColor = ab;",
            "vertex colours lerp between layers",
        ),
        (
            "let ab : ColorLinear = lerp(a, b, r); let aba : ColorLinear = lerp(ab, a, r); output baseColor = aba;",
            "only once",
        ),
        (
            "let ab : ColorLinear = lerp(a, b, r); let abc : ColorLinear = lerp(ab, b, g); let e : ColorLinear = lerp(a, b, bl);
             output baseColor = abc; output emissive = e;",
            "at most 4",
        ),
        ("output roughness = al;", "scalar must be constant"),
    ] {
        let error = graph(body).unwrap_err().to_string();
        assert!(error.contains(expected), "{body}: {error}");
    }
}
