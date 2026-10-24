use super::*;
use ara_asset::{Cancellation, CookPreset, ImportContext, ImportInput, TargetPlatform};
use serde_json::json;

pub(crate) fn scene() -> Value {
    let mut source: Value = serde_json::from_slice(include_bytes!(
        "../../../ara-importers/tests/fixtures/material-extensions.gltf"
    ))
    .expect("fixture");
    source["nodes"] = json!([
        {"name":"Root", "translation":[10,0,20], "children":[1,3]},
        {"name":"PieceA", "translation":[2,0,3], "rotation":[0,0.7071067811865475,0,0.7071067811865475], "scale":[2,2,2], "mesh":0,"children":[2]},
        {"name":"Bolt", "translation":[0,1,0], "mesh":0},
        {"name":"PieceB", "translation":[-4,0,0], "mesh":0},
        {"name":"Unused", "mesh":0},
        {"name":"Empty"}
    ]);
    source["scenes"] = json!([{"nodes":[0,5]}]);
    source
}

pub(crate) fn imported(source: Value) -> CachedImport {
    let registry = ara_importers::registry().expect("registry");
    let importer = registry.get("ara.gltf").expect("glTF");
    let input = ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 4,
        source_path: "kit.gltf".into(),
        source: ara_json::to_canonical_string(&source).into_bytes(),
        importer: importer.id().into(),
        version: importer.version().into(),
        settings: json!({"virtualGeometry":false}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
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
    let key = input.key().expect("key");
    CachedImport { key, output }
}

fn cook(source: Value) -> Model {
    let cached = imported(source);
    Model::decode("as_kit", cached.key, &cached).expect("model")
}

#[test]
fn selection_preserves_subtrees_orientation_scale_and_local_override_numbers() {
    let model = cook(scene());
    assert_eq!(model.primitives.len(), 3);
    let whole = model.select(None, None).expect("whole scene");
    assert_eq!(whole.primitives, 0..3);
    assert_eq!(whole.offset, [0.; 3]);
    let a = model.select(Some("PieceA"), None).expect("subtree");
    assert_eq!(a.primitives, 0..2);
    assert_eq!(a.offset, [-12., 0., -23.]);
    let bolt = model.select(Some("2"), None).expect("source node index");
    assert_eq!(bolt.primitives, 1..2);
    assert_eq!(bolt.base, 1);
    assert_eq!(bolt.offset, [-12., -2., -23.]);
    let filtered = model
        .select(Some("PieceA"), Some(1))
        .expect("subtree primitive");
    assert_eq!(filtered.primitives, 1..2);
    assert_eq!(
        filtered.base, 0,
        "material override slots precede primitive filtering"
    );
    let b = model.select(Some("PieceB"), Some(0)).expect("sibling");
    assert_eq!(b.primitives, 2..3);
    assert_eq!(b.base, 2);
    assert_eq!(b.offset, [-6., 0., -20.]);
    let root = model.select(Some("Root"), None).expect("ancestor range");
    assert_eq!(root.primitives, 0..3);
    assert_eq!(root.offset, [-10., 0., -20.]);
    let primitive = &model.primitives[a.primitives.start];
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for vertex in &primitive.vertices {
        let p = Vec3::from_array(vertex.position) + Vec3::from_array(a.offset);
        min = min.min(p);
        max = max.max(p);
    }
    assert!(min.abs_diff_eq(Vec3::new(0., 0., -2.), 0.005), "{min:?}");
    assert!(max.abs_diff_eq(Vec3::new(2., 0., 0.), 0.005), "{max:?}");
    assert!(model.select(Some("PieceA"), Some(2)).is_err());
    assert!(model.select(None, Some(3)).is_err());
    assert!(
        model
            .select(Some("Empty"), None)
            .expect_err("empty")
            .0
            .contains("no drawable")
    );
    assert!(
        model.select(Some("Unused"), None).is_err(),
        "unreachable scenes stay excluded"
    );
    assert!(model.select(Some("999"), None).is_err());
    let mut renderer = crate::components::MeshRenderer {
        node: Some("PieceA".into()),
        emissive_factor: Some([1.; 3]),
        emissive_materials: vec!["Layered".into(), "0".into()],
        ..Default::default()
    };
    model
        .validate_renderer(&renderer)
        .expect("valid material selectors");
    renderer.emissive_materials = vec!["MissingMaterial".into()];
    assert!(
        model
            .validate_renderer(&renderer)
            .expect_err("invalid material selector")
            .0
            .contains("MissingMaterial")
    );
}

#[test]
fn duplicate_and_numeric_node_names_have_unambiguous_index_escapes() {
    let mut source = scene();
    source["nodes"][3]["name"] = json!("PieceA");
    let model = cook(source);
    assert!(
        model
            .select(Some("PieceA"), None)
            .expect_err("ambiguous")
            .0
            .contains("ambiguous")
    );
    assert_eq!(
        model
            .select(Some("3"), None)
            .expect("numeric index")
            .primitives,
        2..3
    );
    let mut source = scene();
    source["nodes"][1]["name"] = json!("3");
    let model = cook(source);
    assert_eq!(
        model.select(Some("3"), None).expect("index").primitives,
        2..3
    );
    assert_eq!(
        model
            .select(Some("name:3"), None)
            .expect("numeric name")
            .primitives,
        0..2
    );
}

#[test]
fn mirrored_source_parts_preserve_front_faces_and_tangent_handedness() {
    fn winding(primitive: &Primitive) -> f32 {
        let triangle = &primitive.indices[..3];
        let a = Vec3::from_array(primitive.vertices[triangle[0] as usize].position);
        let b = Vec3::from_array(primitive.vertices[triangle[1] as usize].position);
        let c = Vec3::from_array(primitive.vertices[triangle[2] as usize].position);
        (b - a).cross(c - a).dot(Vec3::from_array(
            primitive.vertices[triangle[0] as usize].normal,
        ))
    }
    let regular = cook(scene());
    let mut source = scene();
    source["nodes"][1]["scale"] = json!([-2, 2, 2]);
    let mirrored = cook(source);
    let (regular, mirrored) = (&regular.primitives[0], &mirrored.primitives[0]);
    assert!(winding(regular) * winding(mirrored) > 0.);
    assert_eq!(
        regular.vertices[0].tangent[3],
        -mirrored.vertices[0].tangent[3]
    );
}
