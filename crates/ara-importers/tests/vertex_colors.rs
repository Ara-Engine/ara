//! glTF `COLOR_0` survives the cook as linear RGBA8 in every component type the
//! spec allows (float, normalized u8/u16; RGB and RGBA); meshes without colours
//! cook opaque white.
use ara_asset::{
    ArtifactKind, Cancellation, CookPreset, ImportContext, ImportInput, TargetPlatform,
};
use ara_asset_format::{Mesh, QuantizedVertex};
use base64::Engine;
use serde_json::json;
use std::collections::BTreeMap;

/// Linear RGBA of the triangle's three source vertices.
const COLORS: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 1.0],
    [0.0, 0.5, 0.0, 0.25],
    [0.2, 0.4, 1.0, 0.0],
];

/// One triangle with `COLOR_0` of glTF `component` type and `kind` (`VEC3`/`VEC4`),
/// or without colours.
fn gltf(colors: Option<(u32, &str)>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for p in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
        bytes.extend(p.iter().flat_map(|v| v.to_le_bytes()));
    }
    let positions = bytes.len();
    let mut attributes = json!({"POSITION": 0});
    let mut accessors = vec![
        json!({"bufferView":0,"componentType":5126,"count":3,"type":"VEC3",
        "min":[0,0,0],"max":[1,1,0]}),
    ];
    let mut views = vec![json!({"buffer":0,"byteOffset":0,"byteLength":positions})];
    if let Some((component, kind)) = colors {
        let lanes = if kind == "VEC3" { 3 } else { 4 };
        let start = bytes.len();
        for color in COLORS {
            for &c in &color[..lanes] {
                match component {
                    5126 => bytes.extend(c.to_le_bytes()),
                    5121 => bytes.push((c * 255.).round() as u8),
                    _ => bytes.extend(((c * 65535.).round() as u16).to_le_bytes()),
                }
            }
            // Vertex attributes align each element to four bytes.
            while (bytes.len() - start) % 4 != 0 {
                bytes.push(0);
            }
        }
        let stride = (bytes.len() - start) / 3;
        views.push(json!({"buffer":0,"byteOffset":start,"byteLength":bytes.len() - start,"byteStride":stride}));
        accessors.push(
            json!({"bufferView":1,"componentType":component,"count":3,"type":kind,
            "normalized": component != 5126}),
        );
        attributes["COLOR_0"] = json!(1);
    }
    let uri = format!(
        "data:application/octet-stream;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    serde_json::to_vec(&json!({
        "asset":{"version":"2.0"},
        "buffers":[{"byteLength":bytes.len(),"uri":uri}],
        "bufferViews":views,
        "accessors":accessors,
        "meshes":[{"primitives":[{"attributes":attributes}]}],
        "nodes":[{"mesh":0}],
        "scenes":[{"nodes":[0]}],
        "scene":0
    }))
    .expect("json")
}

/// Cooked RGBA8 colour of each source vertex.
fn cooked(source: Vec<u8>) -> Vec<[u8; 4]> {
    let registry = ara_importers::registry().expect("registry");
    let importer = registry.get("ara.gltf").expect("installed importer");
    let input = ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 2,
        source_path: "triangle.gltf".into(),
        source,
        importer: importer.id().into(),
        version: importer.version().into(),
        settings: json!({"virtualGeometry": false}),
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
    let artifact = output
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Mesh)
        .expect("a cooked mesh");
    let mesh = Mesh::read(&artifact.bytes).expect("valid mesh");
    let mut colors = vec![[0; 4]; 3];
    for (raw, source) in mesh
        .vertices
        .as_chunks::<52>()
        .0
        .iter()
        .zip(mesh.source_vertices.as_chunks::<4>().0)
    {
        let vertex: QuantizedVertex = bytemuck::pod_read_unaligned(raw);
        colors[u32::from_le_bytes(*source) as usize] = vertex.color;
    }
    colors
}

#[test]
fn color_0_cooks_every_component_type_as_linear_rgba8() {
    let expected = |alpha: bool| -> Vec<[u8; 4]> {
        COLORS
            .iter()
            .map(|c| {
                let a = if alpha { c[3] } else { 1. };
                [c[0], c[1], c[2], a].map(|v| (v * 255. + 0.5) as u8)
            })
            .collect()
    };
    for component in [5126, 5121, 5123] {
        for (kind, alpha) in [("VEC3", false), ("VEC4", true)] {
            let colors = cooked(gltf(Some((component, kind))));
            for (got, want) in colors.iter().zip(expected(alpha)) {
                assert!(
                    got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1),
                    "{component}/{kind}: {colors:?}, expected {:?}",
                    expected(alpha)
                );
            }
        }
    }
    assert_eq!(cooked(gltf(None)), [[255; 4]; 3], "no COLOR_0 reads white");
}
