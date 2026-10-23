#![allow(clippy::unwrap_used)]
mod common;
use ara_nav::*;
use common::*;

#[test]
fn rooms_ramp_stairs_bridge_gap_and_straight_path() {
    let input = level();
    let (_, mesh) = bake(&input);
    let mut q = mesh.query().unwrap();
    let filter = QueryFilter::default();
    let path = q.path([3.0, 0.0, 6.0], [56.0, 3.6, 6.0], &filter).unwrap();
    assert!(path.complete, "{path:?}");
    assert!(path.corners.iter().any(|c| c.off_mesh));
    assert!(
        !q.path(
            [3.0, 0.0, 6.0],
            [56.0, 3.6, 6.0],
            &QueryFilter {
                exclude: 2,
                ..filter
            }
        )
        .unwrap()
        .complete
    );
    q.set_extent([0.5, 0.5, 0.5]).unwrap();
    let lower = q.nearest([10.0, 0.0, 18.0], &filter).unwrap().unwrap();
    let bridge = q.nearest([10.0, 4.0, 18.0], &filter).unwrap().unwrap();
    assert!(lower.position[1] < 0.3);
    assert!(bridge.position[1] > 3.9);
    assert!(
        !q.visible([10.0, 0.0, 18.0], [10.0, 4.0, 18.0], &filter)
            .unwrap()
    );
    let bridge_path = q
        .path([38.0, 3.6, 6.0], [10.0, 4.0, 18.0], &filter)
        .unwrap();
    assert!(bridge_path.complete, "{bridge_path:?}");
    let mut debug = Vec::new();
    mesh.debug_stream(|p| debug.push(p));
    assert!(
        debug
            .iter()
            .any(|p| matches!(p, DebugPrimitive::Triangle { .. }))
    );
    assert!(
        debug
            .iter()
            .any(|p| matches!(p, DebugPrimitive::Link { id: 7, .. }))
    );
}

#[test]
fn nearest_raycast_seams_and_parallel_query_order() {
    let (_, mesh) = bake(&plane(32.0));
    let mut q = mesh.query().unwrap();
    let f = QueryFilter::default();
    assert!(
        q.raycast([2.0, 0.0, 2.0], [30.0, 0.0, 2.0], &f)
            .unwrap()
            .fraction
            == 1.0
    );
    assert!(
        q.raycast([2.0, 0.0, 2.0], [50.0, 0.0, 2.0], &f)
            .unwrap()
            .fraction
            < 0.7
    );
    assert!(q.nearest([80.0, 0.0, 80.0], &f).unwrap().is_none());
    let requests: Vec<_> = (0..64)
        .map(|i| PathRequest {
            start: [2.0, 0.0, 2.0],
            end: [3.0 + i as f32 * 0.4, 0.0, 28.0],
            filter: f,
        })
        .collect();
    let sequential: Vec<_> = requests
        .iter()
        .map(|r| q.path(r.start, r.end, &r.filter))
        .collect();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let parallel = pool.install(|| mesh.paths(&requests));
    assert_eq!(sequential, parallel);
    assert!(parallel.iter().all(|p| p.as_ref().unwrap().complete));
}

#[test]
fn area_costs_route_around_expensive_strip_and_explain_blocking_area() {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 8.0], [0.0, 12.0], 0.0, 0);
    floor(&mut g, [8.0, 16.0], [0.0, 8.0], 0.0, 3);
    floor(&mut g, [8.0, 16.0], [8.0, 12.0], 0.0, 0);
    floor(&mut g, [16.0, 24.0], [0.0, 12.0], 0.0, 0);
    let request = input(
        g,
        Bounds {
            min: [0.0, -1.0, 0.0],
            max: [24.0, 1.0, 12.0],
        },
    );
    let (_, mesh) = bake(&request);
    let mut q = mesh.query().unwrap();
    let mut expensive = QueryFilter::default();
    expensive.costs[3] = 50.0;
    let path = q
        .path([2.0, 0.0, 3.0], [22.0, 0.0, 3.0], &expensive)
        .unwrap();
    assert!(path.complete);
    assert!(path.corners.iter().any(|c| c.position[2] > 7.5), "{path:?}");
    let result = why_unreachable(
        &mesh,
        [2.0, 0.0, 3.0],
        [10.0, 0.0, 3.0],
        &QueryFilter {
            exclude: 1 << 3,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.reason, ReachabilityReason::BlockingArea { area: 3 });
    expensive.costs[3] = f32::NAN;
    assert!(
        q.path([2.0, 0.0, 3.0], [22.0, 0.0, 3.0], &expensive)
            .is_err()
    );
}

#[test]
fn incremental_halos_obstacles_streaming_and_determinism() {
    let mut input = plane(32.0);
    let baker = Baker::new(None);
    let initial = baker.bake(&input, None).unwrap();
    let reused = baker.bake(&input, Some(&initial)).unwrap();
    assert_eq!(reused.stats.reused, initial.tiles.len());
    assert_eq!(reused.stats.rebuilt, 0);
    assert_eq!(initial.content_hash(), reused.content_hash());
    input.obstacles.push(Obstacle {
        id: 9,
        bounds: Bounds {
            min: [15.0, -0.5, 0.0],
            max: [17.0, 3.0, 32.0],
        },
        area: Obstacle::CARVE,
    });
    let cut = baker.bake(&input, Some(&initial)).unwrap();
    assert!(cut.stats.rebuilt > 0);
    assert!(cut.stats.reused > 0);
    let mesh = NavMesh::from_artifact(&cut).unwrap();
    let path = mesh
        .query()
        .unwrap()
        .path([2.0, 0.0, 5.0], [29.0, 0.0, 5.0], &Default::default())
        .unwrap();
    assert!(!path.complete);
    input.obstacles.clear();
    let restored = baker.bake(&input, Some(&cut)).unwrap();
    assert_eq!(initial.content_hash(), restored.content_hash());
    // Producer index/iteration changes cannot change the voxelization order or hash.
    input.geometry.triangles.reverse();
    input.geometry.areas.reverse();
    let permuted = baker.bake(&input, None).unwrap();
    assert_eq!(initial.content_hash(), permuted.content_hash());
    assert_eq!(initial.tiles, permuted.tiles);
    let removed = input.tiles.pop_last().unwrap();
    let streamed = baker.bake(&input, Some(&initial)).unwrap();
    assert_eq!(streamed.stats.removed, 1);
    assert!(!streamed.tiles.contains_key(&removed));
}

#[test]
fn geometry_edits_invalidate_both_sides_of_tile_halo() {
    let mut input = plane(32.0);
    let baker = Baker::new(None);
    let initial = baker.bake(&input, None).unwrap();
    // x=15.2 is a seam on this fixed grid; both neighbors must see the edit.
    floor(&mut input.geometry, [14.9, 15.5], [5.0, 7.0], 0.2, 2);
    let edited = baker.bake(&input, Some(&initial)).unwrap();
    for coord in [TileCoord { x: 2, z: 1 }, TileCoord { x: 3, z: 1 }] {
        assert_ne!(initial.tiles[&coord].key, edited.tiles[&coord].key);
    }
    assert!(edited.stats.rebuilt > 1 && edited.stats.reused > 1);
    let full = baker.bake(&input, None).unwrap();
    assert_eq!(edited.tiles, full.tiles);
}

#[test]
fn disabled_link_diagnostics_and_multiple_agent_profiles() {
    let mut input = level();
    input.links[0].enabled = false;
    let (_, mesh) = bake(&input);
    let why = why_unreachable(
        &mesh,
        [3.0, 0.0, 6.0],
        [56.0, 3.6, 6.0],
        &Default::default(),
    )
    .unwrap();
    assert!(matches!(
        why.reason,
        ReachabilityReason::DisabledLink { id: 7 } | ReachabilityReason::MissingLink
    ));
    assert!(why.nearest_reachable.is_some());
    input.settings.agent = AgentProfile {
        id: 2,
        radius: 0.8,
        height: 3.0,
        ..Default::default()
    };
    let (_, large) = bake(&input);
    assert_eq!(large.profile().id, 2);
    assert_ne!(large.content_hash(), mesh.content_hash());
}

#[test]
fn cache_hits_and_corrupt_entries_rebuild() {
    let root = std::env::temp_dir().join(format!("ara-nav-test-cache-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let input = plane(12.0);
    let baker = Baker::new(Some(root.clone()));
    let initial = baker.bake(&input, None).unwrap();
    let second = baker.bake(&input, None).unwrap();
    assert_eq!(second.stats.cache_hits, initial.tiles.len());
    assert_eq!(initial.content_hash(), second.content_hash());
    let tile = initial.tiles.values().next().unwrap();
    std::fs::write(root.join(tile.key.to_string()), b"broken").unwrap();
    let repaired = baker.bake(&input, None).unwrap();
    assert_eq!(repaired.stats.rebuilt, 1);
    assert_eq!(initial.content_hash(), repaired.content_hash());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn schema_and_invalid_geometry_are_checked() {
    let mut db = ara_schema::SchemaDb::new();
    register(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    for name in ["NavAgent", "NavMeshSurface", "NavLink", "NavArea"] {
        assert!(db.get(name).is_some());
    }
    assert_eq!(
        serde_json::from_str::<NavAgent>("{}").unwrap(),
        NavAgent::default()
    );
    assert_eq!(
        serde_json::from_str::<NavLink>("{}").unwrap(),
        NavLink::default()
    );
    let mut input = plane(12.0);
    input.geometry.triangles[0][0] = u32::MAX;
    assert_eq!(
        Baker::new(None).bake(&input, None).unwrap_err().kind,
        "invalidGeometry"
    );
    input.geometry.triangles[0][0] = 0;
    input.settings.cell_size = 0.0;
    assert_eq!(
        Baker::new(None).bake(&input, None).unwrap_err().kind,
        "invalidSettings"
    );
}

#[test]
fn removing_all_geometry_produces_empty_tombstones() {
    let mut input = plane(12.0);
    let baker = Baker::new(None);
    let old = baker.bake(&input, None).unwrap();
    input.geometry = Geometry::default();
    let cleared = baker.bake(&input, Some(&old)).unwrap();
    assert!(cleared.tiles.values().all(|t| t.byte_len() == 0));
    let mesh = NavMesh::from_artifact(&cleared).unwrap();
    assert!(
        mesh.query()
            .unwrap()
            .nearest([3.0, 0.0, 3.0], &Default::default())
            .unwrap()
            .is_none()
    );
}
