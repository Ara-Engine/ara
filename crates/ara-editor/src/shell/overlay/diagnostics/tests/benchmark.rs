//! Opt-in warm-cache/forced-invalidation CPU comparison; no historical baseline claim.
use super::*;
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "explicit CPU cache measurement writes target evidence; no GPU or golden changes"]
fn diagnostic_cache_measurement() {
    let view = view(vec![(
        "e_diagnostic1",
        json!({"Transform":{},"MeshRenderer":{}}),
        None,
    )]);
    let state = state(true, false);
    let mut scene = scene(Mat4::IDENTITY);
    let theme = sheet();
    for index in 1..1000 {
        let world = Mat4::from_translation(Vec3::new(
            (index % 25) as f32 * 0.5 - 6.,
            (index / 25) as f32 * 0.2 - 4.,
            0.,
        ));
        scene.instances.push(ara_render::instanced::Instance {
            entity: 7 + index,
            name: "shared primitive".into(),
            geometry: 0,
            material: 0,
            world,
            previous_world: world,
        });
    }
    let mut results = Vec::new();
    for forced in [false, true] {
        let mut diagnostic = Diagnostics::default();
        let mut overlays = Vec::new();
        diagnostic
            .append(
                &mut overlays,
                &view,
                &state,
                RECT,
                source(&scene, SourceKey::default()),
                &theme,
            )
            .unwrap();
        let start = Instant::now();
        for index in 0..200 {
            overlays.clear();
            let key = SourceKey {
                pose_revision: forced.then_some(index),
                ..Default::default()
            };
            diagnostic
                .append(
                    &mut overlays,
                    &view,
                    &state,
                    RECT,
                    source(&scene, key),
                    &theme,
                )
                .unwrap();
            black_box(&overlays);
        }
        results.push(json!({"mode":if forced{"forced-invalidation"}else{"warm-cache"},"iterations":200,
            "milliseconds":start.elapsed().as_secs_f64()*1000.,"instances":scene.instances.len(),"cache":diagnostic.report()}));
    }
    assert_eq!(results[0]["cache"]["geometryBuilds"], 1);
    assert_eq!(results[0]["cache"]["projectionBuilds"], 1);
    assert_eq!(
        results[0]["cache"]["projectedSegments"],
        results[1]["cache"]["projectedSegments"]
    );
    let target = crate::test_target().join("viewport-diagnostics");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("cache-measurement.json"),ara_json::to_canonical_string(&json!({
        "scope":"Same actual native geometry and output append; warm cache versus forced geometry/projection invalidation, not a historical product baseline", "results":results}))).unwrap();
}
