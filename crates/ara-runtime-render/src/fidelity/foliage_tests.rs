use super::*;
use ara_foliage::{Exclusion, Rule, Species};
use ara_runtime::terrain::TerrainData;
use ara_terrain::{Header, Tile, TileCoord};
use std::sync::Arc;

#[test]
fn foliage_uses_the_rule_model_and_respects_exclusion_zones() {
    let coord = TileCoord { x: 0, z: 0 };
    let data = TerrainData {
        header: Header {
            origin: [0.0; 2],
            spacing: 1.0,
            cells: 16,
            min: coord,
            max: TileCoord { x: 1, z: 1 },
            materials: Vec::new(),
        },
        tiles: BTreeMap::from([(coord, Tile::flat(16).expect("valid tile"))]),
        foliage: Vec::new(),
        foliage_rules: Vec::new(),
    };
    let source = crate::geometry::shape(ara_runtime::components::MeshShape::Plane, [1.0; 3]);
    let entry = Foliage {
        rule: Rule {
            name: "Authored shrub".into(),
            species: Species::Grass,
            asset: ara_schema::AssetRef("@plants/shrub.glb#as_shrub".into()),
            seed: 7,
            cell_size: 4.0,
            density: 1.0 / 16.0,
            height: [-1.0, 1.0],
            slope: [0.0, 90.0],
            channel: 0,
            min_weight: 0.0,
            noise: 0.0,
            scale: [1.0; 2],
            enabled: true,
        },
        model: Arc::new(Model {
            asset: "as_shrub".into(),
            digest: ara_asset::Digest([9; 32]),
            primitives: vec![Primitive {
                vertices: source.vertices,
                indices: source.indices,
                lods: Vec::new(),
                material: None,
                skin: None,
            }],
            materials: Vec::new(),
            textures: Vec::new(),
            skins: Vec::new(),
            bounds: [[-0.5, 0.0, -0.5], [0.5, 0.0, 0.5]],
        }),
    };
    let mut builder = Builder::default();
    builder
        .foliage(std::slice::from_ref(&entry), &[], &data)
        .expect("scatter");
    assert_eq!(builder.stats.foliage_instances, 16);
    let group = builder
        .groups
        .values()
        .next()
        .expect("foliage model geometry");
    assert_eq!(group.name, "Authored shrub");
    assert_eq!(
        group.vertices.len(),
        16 * 4,
        "uses model planes, not species cones"
    );
    assert_eq!(group.indices.len(), 16 * 6);
    assert!(group.vertices.iter().all(|v| v.position[1] == 0.0));

    let mut excluded = Builder::default();
    excluded
        .foliage(
            &[entry],
            &[Exclusion {
                center: [8.0; 2],
                radius: 100.0,
            }],
            &data,
        )
        .expect("scatter with exclusion");
    assert_eq!(excluded.stats.foliage_instances, 0);
    assert!(excluded.groups.is_empty());
}
