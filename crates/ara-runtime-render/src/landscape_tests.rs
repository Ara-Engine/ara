use super::landscape::landscape_data;
use ara_runtime::terrain::TerrainData;
use ara_terrain::{Header, Tile, TileCoord};
use std::collections::BTreeMap;

#[test]
fn atlas_rebases_signed_tile_coordinates_and_preserves_authored_world_origin() {
    let header = Header {
        origin: [125.0, -82.0],
        spacing: 2.0,
        cells: 16,
        min: TileCoord { x: -3, z: 5 },
        max: TileCoord { x: -1, z: 7 },
        materials: Vec::new(),
    };
    let mut tiles = BTreeMap::new();
    for z in 0..2 {
        for x in 0..2 {
            let mut tile = Tile::flat(16).expect("valid tile");
            tile.heights.fill((x + 10 * z) as f32);
            tile.weights.fill([100, 50, 75, 30]);
            tiles.insert(
                TileCoord {
                    x: header.min.x + x,
                    z: header.min.z + z,
                },
                tile,
            );
        }
    }
    let data = TerrainData {
        header,
        tiles,
        foliage: Vec::new(),
        foliage_rules: Vec::new(),
    };
    let atlas = landscape_data(&data).expect("nonzero origin and signed tile bounds");
    assert_eq!(atlas.side, 33);
    assert_eq!(
        atlas.header, data.header,
        "ara-landscape owns the world-space rebase"
    );
    for z in 0..2 {
        for x in 0..2 {
            let texel = atlas.samples[(z * 16 + 8) * 33 + x * 16 + 8];
            assert_eq!(f32::from_bits(texel[0]), (x + 10 * z) as f32);
            assert_eq!(texel[1].to_le_bytes(), [100, 50, 75, 30]);
        }
    }
}
