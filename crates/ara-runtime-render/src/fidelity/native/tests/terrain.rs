use super::*;
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_runtime::{
    animation::Library,
    assets::MemoryAssets,
    extract::TerrainView,
    terrain::{FoliageRule, TerrainData},
};
use ara_schema::AssetRef;
use ara_terrain::{Header, Tile, TileCoord};

#[test]
fn signed_terrain_splats_and_rule_prefab_parts_use_runtime_placements_once() {
    let coord = TileCoord { x: -1, z: 2 };
    let mut tile = Tile::flat(16).unwrap();
    tile.weights.fill([10, 20, 30, 195]);
    tile.holes[0] = 1;
    let rule = ara_foliage::Rule {
        name: "Composite".into(),
        species: ara_foliage::Species::Rock,
        asset: AssetRef("@parts.prefab#pf_parts".into()),
        seed: 1,
        cell_size: 4.0,
        density: 0.01,
        height: [-10.0, 10.0],
        slope: [0.0, 90.0],
        channel: 0,
        min_weight: 0.0,
        noise: 0.0,
        scale: [1.0, 1.0],
        enabled: true,
    };
    let mut disabled = rule.clone();
    disabled.enabled = false;
    let data = TerrainData {
        header: Header {
            origin: [10.0, 20.0],
            spacing: 1.0,
            cells: 16,
            min: coord,
            max: TileCoord { x: 0, z: 3 },
            materials: Vec::new(),
        },
        tiles: BTreeMap::from([(coord, tile)]),
        foliage: vec![ara_foliage::Instance {
            position_scale: [-5.0, 0.0, 53.0, 2.0],
            rotation_wind: [0.0; 4],
            identity: [7, 9, 2, 1],
        }],
        foliage_rules: vec![
            FoliageRule {
                id: "fo_test#r_disabled".into(),
                rule: disabled,
            },
            FoliageRule {
                id: "fo_test#r_parts".into(),
                rule,
            },
        ],
    };
    let mut frame = RenderFrame::default();
    frame.terrain = Some(TerrainView {
        data: Arc::new(data),
        tiles: vec![coord],
    });
    let mut prefab = Document::new("prefab", AssetId::parse("pf_parts").unwrap());
    for (id, record) in [
        (
            "e_root",
            serde_json::json!({"name":"Root","Transform":{"position":[1,0,0]},"MeshRenderer":{}}),
        ),
        (
            "e_tip",
            serde_json::json!({"name":"Tip","parent":"e_root","Transform":{"position":[0,2,0]},"MeshRenderer":{}}),
        ),
    ] {
        prefab.insert_unchecked(
            RecordId::parse(id).unwrap(),
            record.as_object().unwrap().clone(),
        );
    }
    let mut workspace = Workspace::default();
    workspace.insert("parts.prefab", prefab);
    let mut content = Content::resolve(
        &frame,
        &workspace,
        &ara_runtime::schema(&[]).unwrap(),
        &Library::default(),
        &MemoryAssets::default(),
    )
    .unwrap();
    {
        let original = workspace.doc("pf_parts").unwrap().clone();
        let mut missing = original.clone();
        let mut record = missing.record("e_tip").unwrap().clone();
        record.insert(
            "MeshRenderer".into(),
            serde_json::json!({"model":"@missing.gltf#as_missing"}),
        );
        missing.insert_unchecked(RecordId::parse("e_tip").unwrap(), record);
        workspace.insert("parts.prefab", missing);
        let schema = ara_runtime::schema(&[]).unwrap();
        let library = Library::default();
        let source = MemoryAssets::default();
        let pending = Content::resolve_available_with_pending(
            &frame,
            &workspace,
            &schema,
            &library,
            &source,
            &|_| true,
        )
        .unwrap();
        let parts = &pending.foliage_parts["@parts.prefab#pf_parts"];
        assert_eq!(
            parts.iter().find(|p| p.address == "e_tip").unwrap().color,
            crate::PENDING_MODEL_COLOR
        );
        let mut prepared = prepare(&frame, pending, config()).unwrap();
        prepared.enable_motion();
        let slots = &prepared.unpublished_materials["as_missing"];
        assert_eq!(slots.len(), 1);
        assert_eq!(
            prepared.scene.materials[slots[0]].pbr.base_color,
            crate::PENDING_MODEL_COLOR
        );
        let failed = Content::resolve_available_with_pending(
            &frame,
            &workspace,
            &schema,
            &library,
            &source,
            &|_| false,
        )
        .unwrap();
        assert_eq!(
            failed.foliage_parts["@parts.prefab#pf_parts"]
                .iter()
                .find(|p| p.address == "e_tip")
                .unwrap()
                .color,
            [1., 0., 1., 1.]
        );
        workspace.insert("parts.prefab", original);
    }
    let data = Arc::get_mut(&mut frame.terrain.as_mut().unwrap().data).unwrap();
    for (i, color) in [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
        [0.3, 0.4, 0.5, 1.0],
    ]
    .into_iter()
    .enumerate()
    {
        let reference = format!("@surface{i}.graph#gr_surface{i}");
        data.header.materials.push(AssetRef(reference.clone()));
        content.terrain_materials.insert(
            reference,
            Arc::new(MaterialAsset {
                asset: format!("gr_surface{i}"),
                digest: AssetDigest::of(&[i as u8]),
                material: ModelMaterial {
                    base_color: color,
                    ..Default::default()
                },
                textures: Vec::new(),
                texture_generations: Vec::new(),
                base_color_decode_srgb: false,
                emissive_decode_srgb: false,
            }),
        );
    }
    let prepared = prepare(&frame, content, config()).unwrap();
    assert_eq!(
        prepared.scene.geometry.len(),
        2,
        "one tile and one shared box"
    );
    assert_eq!(
        prepared.scene.instances.len(),
        3,
        "one tile and the two authored parts"
    );
    let tile = &prepared.scene.geometry[0];
    assert_eq!(tile.vertices[0].position, [-6.0, 0.0, 52.0]);
    assert_eq!(
        tile.vertices[0].terrain_weights,
        u32::from_le_bytes([10, 20, 30, 195])
    );
    assert_eq!(tile.vertices[0].tangent[3], -1.0);
    assert_eq!(
        tile.lods[0].indices.len(),
        16 * 16 * 6 - 6,
        "the authored hole remains empty"
    );
    let material = &prepared.scene.materials[prepared.scene.instances[0].material];
    assert_eq!(material.extras.terrain_layers, 4);
    let blue = material.extras.layer_materials[2] as usize;
    assert_eq!(
        prepared.scene.materials[blue].pbr.base_color,
        [0.0, 0.0, 1.0, 1.0]
    );
    let positions: Vec<_> = prepared.scene.instances[1..]
        .iter()
        .map(|i| i.world.w_axis.truncate().to_array())
        .collect();
    assert_eq!(positions, [[-3.0, 0.0, 53.0], [-3.0, 4.0, 53.0]]);
    assert_eq!(
        prepared.scene.instances[1].geometry,
        prepared.scene.instances[2].geometry
    );
}
