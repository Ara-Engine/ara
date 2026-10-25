use super::*;
use ara_doc::Workspace;
use ara_runtime::{
    animation::Library, assets::MemoryAssets, extract::TerrainView, terrain::TerrainData,
};
use ara_schema::AssetRef;
use ara_terrain::{Header, TileCoord};

fn graph(workspace: &mut Workspace, body: &str) {
    let definition = ara_material::dsl::parse(
        &format!("material Ground : surface budget 1 {{ {body} }}"),
        "gr_ground",
    )
    .unwrap();
    workspace.insert(
        "ground.graph",
        definition
            .to_graph(Default::default())
            .unwrap()
            .to_document()
            .unwrap(),
    );
}

fn fixture() -> (RenderFrame, Workspace) {
    let mut index = Workspace::default().index().clone();
    index.insert("as_image", "image.png", Some("import".into()));
    let mut workspace = Workspace::new(index);
    graph(
        &mut workspace,
        "param image : TextureColor = @image.png#as_image; let coords : vec2 = uv(); let base : ColorLinear = sampleColor(image, coords); output baseColor = base;",
    );
    let mut frame = RenderFrame::default();
    frame.terrain = Some(TerrainView {
        data: Arc::new(TerrainData {
            header: Header {
                origin: [0.; 2],
                spacing: 1.,
                cells: 16,
                min: TileCoord { x: 0, z: 0 },
                max: TileCoord { x: 1, z: 1 },
                materials: vec![AssetRef("@ground.graph#gr_ground".into())],
            },
            tiles: BTreeMap::new(),
            foliage: Vec::new(),
            foliage_rules: Vec::new(),
        }),
        tiles: Vec::new(),
    });
    (frame, workspace)
}

#[test]
fn pending_terrain_is_neutral_dielectric_then_uses_the_published_maps() {
    let (frame, workspace) = fixture();
    let schema = ara_runtime::schema(&[]).unwrap();
    let library = Library::default();
    let mut source = MemoryAssets::default();
    let content =
        Content::resolve_available(&frame, &workspace, &schema, &library, &source).unwrap();
    let pending = &content.terrain_materials["@ground.graph#gr_ground"];
    assert_eq!(pending.material.base_color, [0.2, 0.2, 0.2, 1.]);
    assert_eq!(pending.material.roughness, 1.);
    assert_eq!(pending.material.metallic, 0.);
    assert_eq!(pending.material.occlusion_strength, 1.);
    assert!(pending.textures.is_empty());
    assert!(Content::resolve(&frame, &workspace, &schema, &library, &source).is_err());

    source.0.insert(
        "as_image".into(),
        ara_asset::CachedImport {
            key: AssetDigest::of(b"published"),
            output: ara_asset::ImportOutput {
                artifacts: vec![ara_asset::Artifact {
                    name: "texture.ktx2".into(),
                    kind: ara_asset::ArtifactKind::Texture,
                    bytes: write_ktx2(4, 4, TextureFormat::Bc7Srgb, &[vec![0; 16]]).unwrap(),
                }],
            },
        },
    );
    let content =
        Content::resolve_available(&frame, &workspace, &schema, &library, &source).unwrap();
    let ready = &content.terrain_materials["@ground.graph#gr_ground"];
    assert_eq!(ready.material.base_color_texture, Some(0));
    assert_eq!(ready.textures.len(), 1);
    assert_ne!(ready.digest, pending.digest);
    assert!(Content::resolve(&frame, &workspace, &schema, &library, &source).is_ok());
}

#[test]
fn pending_terrain_reimport_keeps_known_authored_factors_and_last_good_material() {
    let (frame, textured) = fixture();
    let mut workspace = textured.clone();
    graph(
        &mut workspace,
        "param tint : ColorLinear = linear(0.31, 0.42, 0.53); let rough : float = 0.8; let metal : float = 0; output baseColor = tint; output roughness = rough; output metallic = metal;",
    );
    let source = MemoryAssets::default();
    let mut library = Library::default();
    library
        .materials
        .load(&workspace, &source, "gr_ground")
        .unwrap();
    let known = library.materials.get("gr_ground").unwrap();
    let content = Content::resolve_available(
        &frame,
        &textured,
        &ara_runtime::schema(&[]).unwrap(),
        &library,
        &source,
    )
    .unwrap();
    let pending = &content.terrain_materials["@ground.graph#gr_ground"];
    assert!(Arc::ptr_eq(known, pending));
    assert_eq!(pending.material.base_color, [0.31, 0.42, 0.53, 1.]);
    assert_eq!(pending.material.roughness, 0.8);
    assert_eq!(pending.material.metallic, 0.);
}
