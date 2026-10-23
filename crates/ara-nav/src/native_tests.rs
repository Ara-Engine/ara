//! Private byte-format rejection tests at the unsafe boundary.
#![allow(clippy::unwrap_used)]
use crate::*;
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn cached_native_headers_and_vertices_are_validated_before_queries() {
    let settings = BakeSettings::default();
    let coord = TileCoord { x: 0, z: 0 };
    let geometry = Geometry {
        vertices: vec![
            [0.0, 0.0, 0.0],
            [12.0, 0.0, 0.0],
            [12.0, 0.0, 12.0],
            [0.0, 0.0, 12.0],
        ],
        triangles: vec![[0, 2, 1], [0, 3, 2]],
        areas: vec![0; 2],
    };
    let bytes = crate::native::bake(&settings, coord, &geometry, &[], &[]).unwrap();
    assert!(bytes.len() > 104);
    let make = |bytes: Vec<u8>| BakeArtifact {
        settings: settings.clone(),
        tiles: BTreeMap::from([(
            coord,
            TileData {
                key: CacheKey([0; 32]),
                bytes: Arc::from(bytes),
            },
        )]),
        links: vec![],
        stats: Default::default(),
    };
    assert!(NavMesh::from_artifact(&make(bytes.clone())).is_ok());
    // Pinned Detour 1.6 header offsets: magic, version, array counts, dimensions,
    // bounds, BV quantization and the first vertex. -1 counts / NaNs must reject.
    for offset in [
        0, 4, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 96, 100,
    ] {
        let mut corrupt = bytes.clone();
        corrupt[offset..offset + 4].fill(255);
        assert_eq!(
            NavMesh::from_artifact(&make(corrupt)).unwrap_err().kind,
            "invalidTile",
            "offset {offset}"
        );
    }
}
