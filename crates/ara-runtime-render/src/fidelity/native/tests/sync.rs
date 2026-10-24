use super::*;
use ara_runtime::model::{Model, ModelVertex, Primitive};

#[test]
fn live_root_factors_and_poses_do_not_change_shared_vertices_or_sibling_materials() {
    let mut frame = RenderFrame::default();
    frame.draws = vec![draw(1), draw(2)];
    let mut prepared = prepare(&frame, Content::default(), config()).unwrap();
    prepared.enable_motion();
    let pointer = prepared.scene.geometry[0].vertices.as_ptr();
    frame.draws[0].world.w_axis.x = 5.0;
    frame.draws[0].color = [0.25, 0.5, 0.75, 1.0];
    frame.draws[0].emissive = [0.2, 0.3, 0.4];
    frame.environment.exposure = 0.005;
    assert_eq!(
        prepared.update_frame(&frame).unwrap(),
        FrameChange::Ready { poses: 1, skins: 0 }
    );
    assert_eq!(prepared.scene.geometry[0].vertices.as_ptr(), pointer);
    assert_eq!(prepared.scene.instances[0].world.w_axis.x, 5.0);
    assert_eq!(prepared.changes().instances, [0]);
    assert_eq!(
        prepared.changes().materials,
        [prepared.scene.instances[0].material]
    );
    let first = &prepared.scene.materials[prepared.scene.instances[0].material];
    let second = &prepared.scene.materials[prepared.scene.instances[1].material];
    assert_eq!(first.pbr.base_color, [0.25, 0.5, 0.75, 1.0]);
    assert_eq!(first.pbr.emissive_roughness[..3], [0.2, 0.3, 0.4]);
    assert_eq!(second.pbr.base_color, [1.0; 4]);
    assert_eq!(second.pbr.emissive_roughness[..3], [2.0, 3.0, 4.0]);
    assert_eq!(
        prepared.update_frame(&frame).unwrap(),
        FrameChange::Ready { poses: 0, skins: 0 }
    );
    assert!(prepared.changes().instances.is_empty());
    assert!(prepared.changes().materials.is_empty());
    assert!(prepared.changes().geometries.is_empty());
    frame.draws[0].entity = 99;
    assert_eq!(prepared.update_frame(&frame).unwrap(), FrameChange::Rebuild);
}

#[test]
fn changed_palettes_update_only_skin_attributes_and_keep_original_uv_sets() {
    let vertices = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        .map(|position| ModelVertex {
            position,
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.1, 0.2],
            uv1: [0.8, 0.1],
            weights: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ..Default::default()
        })
        .to_vec();
    let model = Arc::new(Model {
        asset: "as_skin".into(),
        digest: AssetDigest::of(b"skin"),
        primitives: vec![Primitive {
            vertices,
            indices: vec![0, 1, 2],
            lods: Vec::new(),
            material: None,
            skin: Some(0),
        }],
        nodes: BTreeMap::new(),
        node_names: BTreeMap::new(),
        materials: Vec::new(),
        textures: Vec::new(),
        skins: Vec::new(),
        bounds: [[0.0; 3], [1.0, 1.0, 0.0]],
    });
    let mut frame = RenderFrame::default();
    let mut actor = draw(1);
    actor.geometry = Geometry::Model {
        model: Arc::clone(&model),
        primitive: 0,
    };
    actor.skin = Some(0..1);
    frame.draws.push(actor);
    frame.palettes.push(glam::Mat4::IDENTITY);
    let mut prepared = prepare(&frame, Content::default(), config()).unwrap();
    prepared.enable_motion();
    let handles = Arc::strong_count(&model);
    let pointer = prepared.scene.geometry[0].vertices.as_ptr();
    frame.palettes[0] = glam::Mat4::from_translation(glam::Vec3::new(0.25, 0.0, 0.0));
    assert_eq!(
        prepared.update_frame(&frame).unwrap(),
        FrameChange::Ready { poses: 0, skins: 1 }
    );
    let geometry = &prepared.scene.geometry[0];
    assert_eq!(geometry.vertices.as_ptr(), pointer);
    assert_eq!(geometry.vertices[0].position, [0.25, 0.0, 0.0]);
    assert_eq!(geometry.vertices[0].uv, [0.1, 0.2]);
    assert_eq!(geometry.vertices[0].uv1, [0.8, 0.1]);
    assert_eq!(geometry.bounds[1].x, 1.25);
    assert_eq!(geometry.revision, 1);
    assert_eq!(Arc::strong_count(&model), handles);
    assert_eq!(
        prepared.update_frame(&frame).unwrap(),
        FrameChange::Ready { poses: 0, skins: 0 }
    );
    assert_eq!(prepared.scene.geometry[0].revision, 1);
}
