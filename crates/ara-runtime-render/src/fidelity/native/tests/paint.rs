//! Vertex colours and per-instance paint reach the native scene: geometry colours,
//! instance paint streams, the glTF tint flag and vertex-paint layer materials.
use super::*;
use ara_frame::scene::{VERTEX_COLOR_TINT, VERTEX_PAINT_LAYERS};
use ara_runtime::{
    material::PaintLayer,
    model::{Model, ModelVertex, Primitive},
};

fn coloured_model() -> Arc<Model> {
    let vertices = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        .map(|position| ModelVertex {
            position,
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            color: [255, 128, 0, 255],
            ..Default::default()
        })
        .to_vec();
    Arc::new(Model {
        asset: "as_coloured".into(),
        digest: AssetDigest::of(b"coloured triangle"),
        primitives: vec![Primitive {
            vertices,
            indices: vec![0, 1, 2],
            lods: Vec::new(),
            material: Some(0),
            skin: None,
            casts_shadows: true,
            source_vertices: vec![0, 1, 2],
            vertex_colors: true,
        }],
        nodes: BTreeMap::new(),
        node_names: BTreeMap::new(),
        materials: vec![ModelMaterial::default()],
        textures: Vec::new(),
        skins: Vec::new(),
        bounds: [[0.0; 3], [1.0, 1.0, 0.0]],
    })
}

fn model_draw(entity: u64, model: &Arc<Model>) -> Draw {
    let mut draw = draw(entity);
    draw.geometry = Geometry::Model {
        model: Arc::clone(model),
        primitive: 0,
    };
    draw
}

#[test]
fn imported_colours_and_paint_reach_geometry_and_instances() {
    let model = coloured_model();
    let mut frame = RenderFrame::default();
    frame.draws.push(model_draw(1, &model));
    let mut painted = model_draw(2, &model);
    painted.paint = Some(vec![u32::from_le_bytes([0, 255, 0, 255]); 3].into());
    frame.draws.push(painted);
    // A procedural box whose paint does not cover its 24 vertices draws unpainted.
    let mut shape = draw(3);
    shape.paint = Some(vec![0; 3].into());
    frame.draws.push(shape);
    let mut prepared = prepare(&frame, Content::default(), config()).unwrap();
    let scene = &prepared.scene;
    let geometry = &scene.geometry[scene.instances[0].geometry];
    assert!(geometry.vertex_colors);
    assert_eq!(
        geometry.vertices[0].color,
        u32::from_le_bytes([255, 128, 0, 255])
    );
    assert!(!scene.geometry[scene.instances[2].geometry].vertex_colors);
    assert_eq!(
        scene.instances[0].geometry, scene.instances[1].geometry,
        "paint never copies geometry"
    );
    assert!(scene.instances[0].paint.is_none());
    assert_eq!(
        scene.instances[1].paint.as_deref().map(<[u32]>::len),
        Some(3)
    );
    assert!(scene.instances[2].paint.is_none());
    let material = scene.materials[scene.instances[0].material];
    assert_ne!(
        material.pbr.flags[1] & VERTEX_COLOR_TINT,
        0,
        "glTF COLOR_0 tints"
    );

    // A live paint edit streams; adding paint rebuilds (draws regroup).
    prepared.enable_motion();
    frame.draws[1].paint = Some(vec![u32::from_le_bytes([0, 0, 255, 255]); 3].into());
    assert!(matches!(
        prepared.update_frame(&frame).unwrap(),
        FrameChange::Ready { .. }
    ));
    assert_eq!(prepared.changes().paint, [1]);
    frame.draws[0].paint = Some(vec![0; 3].into());
    assert_eq!(prepared.update_frame(&frame).unwrap(), FrameChange::Rebuild);
}

#[test]
fn paint_layers_become_layer_materials_with_channel_weights() {
    let model = coloured_model();
    let layer = |terms, base: [f32; 4]| PaintLayer {
        terms,
        material: ModelMaterial {
            base_color: base,
            ..ModelMaterial::default()
        },
        base_color_decode_srgb: false,
        emissive_decode_srgb: false,
    };
    let asset = Arc::new(MaterialAsset {
        asset: "gr_layers".into(),
        digest: AssetDigest::of(b"layers"),
        material: ModelMaterial::default(),
        textures: Vec::new(),
        texture_generations: Vec::new(),
        base_color_decode_srgb: false,
        emissive_decode_srgb: false,
        paint_layers: vec![
            layer([2, 0, 0, 0], [1.0, 0.0, 0.0, 1.0]),
            layer([1, 0, 0, 0], [0.0, 0.0, 1.0, 1.0]),
        ],
        decal: None,
    });
    let mut layered = model_draw(1, &model);
    layered.material = Some(asset);
    let mut frame = RenderFrame::default();
    frame.draws.push(layered);
    let prepared = prepare(&frame, Content::default(), config()).unwrap();
    let scene = &prepared.scene;
    let parent = scene.materials[scene.instances[0].material];
    assert_ne!(parent.pbr.flags[1] & VERTEX_PAINT_LAYERS, 0);
    assert_eq!(parent.extras.terrain_layers, 2);
    // Layer 0 weighs 1 - R (bits 0-1 = 2), layer 1 weighs R (bits 8-9 = 1).
    assert_eq!(parent.pbr.flags[2], 2 | 1 << 8);
    let [a, b, ..] = parent.extras.layer_materials;
    assert_eq!(
        scene.materials[a as usize].pbr.base_color,
        [1.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        scene.materials[b as usize].pbr.base_color,
        [0.0, 0.0, 1.0, 1.0]
    );
}

#[test]
fn decals_map_to_native_decals_and_stream_their_poses() {
    use ara_runtime::{
        decal::{Decal, DecalView},
        material::DecalSurface,
    };
    let asset = Arc::new(MaterialAsset {
        asset: "gr_decal".into(),
        digest: AssetDigest::of(b"decal"),
        material: ModelMaterial {
            base_color: [0.5, 0.5, 0.5, 1.0],
            ..ModelMaterial::default()
        },
        textures: Vec::new(),
        texture_generations: Vec::new(),
        base_color_decode_srgb: false,
        emissive_decode_srgb: false,
        paint_layers: Vec::new(),
        decal: Some(DecalSurface {
            opacity: 0.75,
            opacity_texture: None,
            occlusion: 1.0,
            occlusion_channel: 0,
            specular: 0.5,
        }),
    });
    let view = |x: f32, material: Option<Arc<MaterialAsset>>| DecalView {
        entity: 7,
        address: "e_decal".into(),
        label: "Decal (e_decal)".into(),
        world: glam::Mat4::from_translation(glam::Vec3::new(x, 0.0, 0.0)),
        decal: Decal {
            color: ara_schema::Color([1.0, 0.5, 1.0, 0.5]),
            ..Decal::default()
        },
        material,
    };
    let mut frame = RenderFrame::default();
    frame.draws.push(draw(1));
    frame.decals.push(view(0.0, Some(Arc::clone(&asset))));
    frame.decals.push(view(1.0, None));
    let mut prepared = prepare(&frame, Content::default(), config()).unwrap();
    let scene = &prepared.scene;
    assert_eq!(
        scene.decals.len(),
        1,
        "a decal without its material draws nothing"
    );
    let decal = &scene.decals[0];
    assert_eq!(decal.opacity[0], 0.5, "colour alpha scales every channel");
    assert_eq!(decal.opacity_channel, 4);
    let row = scene.materials[decal.material];
    assert_eq!(
        row.pbr.base_color,
        [0.5, 0.25, 0.5, 0.75],
        "tint and opacity factor"
    );
    assert!(scene.instances[0].receives_decals);

    prepared.enable_motion();
    frame.decals[0] = view(2.0, Some(Arc::clone(&asset)));
    assert!(matches!(
        prepared.update_frame(&frame).unwrap(),
        FrameChange::Ready { .. }
    ));
    assert!(prepared.changes().decals);
    assert_eq!(prepared.scene.decals[0].world.w_axis.x, 2.0);
    frame.decals[0].decal.color.0[0] = 0.25;
    assert_eq!(
        prepared.update_frame(&frame).unwrap(),
        FrameChange::Rebuild,
        "colour is in the row"
    );
}
