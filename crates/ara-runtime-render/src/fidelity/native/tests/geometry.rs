use super::*;
use ara_runtime::model::{Model, ModelLod, ModelVertex, Primitive};

fn independent_lods() -> RenderFrame {
    let vertices = [
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [0.0, 0.002, 0.0],
        [2.0, 0.002, 0.0],
        [0.0, 0.004, 0.0],
        [2.0, 0.004, 0.0],
    ]
    .map(|position| ModelVertex {
        position,
        normal: [0.0, 0.0, 1.0],
        tangent: [1.0, 0.0, 0.0, 1.0],
        ..Default::default()
    })
    .to_vec();
    let model = Arc::new(Model {
        asset: "as_independent_lods".into(),
        digest: AssetDigest::of(b"independent lod errors"),
        primitives: vec![Primitive {
            vertices,
            indices: vec![0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4],
            lods: vec![
                ModelLod {
                    indices: vec![0, 1, 3, 0, 3, 2],
                    error: 0.009_922_831,
                },
                ModelLod {
                    indices: vec![0, 1, 3],
                    error: 0.009_920_36,
                },
            ],
            material: None,
            skin: None,
        }],
        nodes: BTreeMap::new(),
        node_names: BTreeMap::new(),
        materials: Vec::new(),
        textures: Vec::new(),
        skins: Vec::new(),
        bounds: [[0.0; 3], [2.0, 0.004, 0.0]],
    });
    let mut actor = draw(1);
    actor.geometry = Geometry::Model {
        model,
        primitive: 0,
    };
    let mut frame = RenderFrame::default();
    frame.draws.push(actor);
    frame
}

#[test]
fn independent_cook_errors_keep_all_lods_with_conservative_bounds() {
    // These estimates reproduce Outpost's Barrel 01 cook: a coarser independent
    // target has a slightly lower error, while both topologies remain valid.
    let frame = independent_lods();
    let prepared = prepare(&frame, Content::default(), config()).unwrap();
    let lods = &prepared.scene.geometry[0].lods;
    assert_eq!(
        lods.iter().map(|lod| lod.indices.len()).collect::<Vec<_>>(),
        [12, 6, 3]
    );
    assert_eq!(lods[0].error, 0.0);
    assert_eq!(lods[1].error, 0.009_922_831 * 2.0);
    assert_eq!(lods[2].error, lods[1].error);
    assert_eq!(lods[2].indices, [0, 1, 3]);
}

#[test]
fn conservative_lod_bounds_still_reject_invalid_cooked_errors() {
    for error in [f32::NAN, f32::INFINITY, -0.01] {
        let mut frame = independent_lods();
        let Geometry::Model { model, .. } = &mut frame.draws[0].geometry else {
            panic!("model fixture");
        };
        Arc::get_mut(model).unwrap().primitives[0].lods[1].error = error;
        assert!(prepare(&frame, Content::default(), config()).is_err());
    }
}
