//! CPU-only iteration probes. Run with `cargo run -p ara-runtime --profile perf
//! --example iteration_bench`. Reference paths retain the pre-optimization work.
use ara_components::Transform;
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_runtime::{math::Pose, sync};
use ara_schema::SchemaDb;
use bevy_ecs::prelude::*;
use std::{hint::black_box, time::Instant};

fn measure(name: &str, mut run: impl FnMut()) {
    for _ in 0..8 {
        run();
    }
    let mut samples = Vec::with_capacity(40);
    for _ in 0..40 {
        let start = Instant::now();
        run();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({"case": name, "p50_ms": samples[20], "p95_ms": samples[37], "samples_ms": samples})
    );
}

// Original end-of-tick path, including both tree maps and component insertion.
fn reference_poses(world: &mut World) {
    for (entity, matrix) in sync::world_matrices(world) {
        let pose = sync::WorldPose(Pose::from_matrix(matrix));
        let mut entity = world.entity_mut(entity);
        if entity.contains::<sync::WorldPose>() {
            entity.insert(pose);
        } else {
            entity.insert((pose, sync::PreviousPose(pose.0)));
        }
    }
}

fn poses(tree: bool, reverse: bool) {
    let mut world = World::new();
    let entities: Vec<_> = (0..10_000)
        .map(|i| {
            world
                .spawn(Transform {
                    position: [i as f32 * 0.01, 1.0, 2.0],
                    rotation: [1.0, 2.0, 3.0],
                    ..Default::default()
                })
                .id()
        })
        .collect();
    if tree {
        for (i, &entity) in entities.iter().enumerate().skip(1) {
            world
                .entity_mut(entity)
                .insert(ChildOf(entities[(i - 1) / 4]));
        }
    }
    let mut reference = Schedule::default();
    reference.set_executor(bevy_ecs::schedule::SingleThreadedExecutor::new());
    reference.add_systems(reference_poses);
    let mut optimized = Schedule::default();
    optimized.set_executor(bevy_ecs::schedule::SingleThreadedExecutor::new());
    optimized.add_systems(sync::resolve_poses_cached);
    reference.run(&mut world);
    let expected: Vec<_> = entities
        .iter()
        .map(|&e| world.get::<sync::WorldPose>(e).expect("resolved").0)
        .collect();
    optimized.run(&mut world);
    for (&entity, expected) in entities.iter().zip(expected) {
        assert_eq!(
            world.get::<sync::WorldPose>(entity).expect("resolved").0,
            expected
        );
    }
    let prefix = if tree { "tree_10k" } else { "flat_10k" };
    for baseline in if reverse {
        [false, true]
    } else {
        [true, false]
    } {
        let schedule = if baseline {
            &mut reference
        } else {
            &mut optimized
        };
        let suffix = if baseline { "reference" } else { "optimized" };
        measure(&format!("{prefix}_{suffix}"), || {
            // All objects move: every transform must be read and resolved.
            for &entity in &entities {
                world
                    .get_mut::<Transform>(entity)
                    .expect("transform")
                    .position[1] += 0.001;
            }
            schedule.run(&mut world);
            black_box(world.get::<sync::WorldPose>(entities[9999]));
        });
    }
}

fn scene(reverse: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut schema = SchemaDb::new();
    ara_doc::register(&mut schema);
    ara_components::register(&mut schema);
    let prefab_id = AssetId::parse("pf_12345678")?;
    let mut prefab = Document::new("prefab", prefab_id);
    for i in 0..8 {
        let mut row = serde_json::Map::new();
        row.insert(
            "Transform".into(),
            serde_json::json!({"position": [i, 1, 2]}),
        );
        if i > 0 {
            row.insert("parent".into(), "e_0000".into());
        }
        prefab.insert_unchecked(RecordId::parse(&format!("e_{i:04}"))?, row);
    }
    let scene_id = AssetId::parse("sc_12345678")?;
    let mut scene = Document::new("scene", scene_id.clone());
    for i in 0..1024 {
        let mut row = serde_json::Map::new();
        row.insert("prefab".into(), "@prefab.prefab#pf_12345678".into());
        scene.insert_unchecked(RecordId::parse(&format!("e_{i:04}"))?, row);
    }
    let mut workspace = Workspace::default();
    workspace.insert("prefab.prefab", prefab);
    workspace.insert("scene.scene", scene);
    let (rows, diagnostics) = workspace.check_resolved(&scene_id, None, &schema);
    assert!(diagnostics.iter().all(|d| !d.is_error()), "{diagnostics:?}");
    assert_eq!(rows.len(), 8192);
    for baseline in if reverse {
        [false, true]
    } else {
        [true, false]
    } {
        measure(
            if baseline {
                "scene_8k_reference"
            } else {
                "scene_8k_optimized"
            },
            || {
                if baseline {
                    let mut diagnostics = workspace.check(&scene_id, None, &schema);
                    black_box(workspace.resolve(&scene_id, &schema, &mut diagnostics));
                    black_box(diagnostics);
                } else {
                    black_box(workspace.check_resolved(&scene_id, None, &schema));
                }
            },
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reverse = std::env::args().any(|arg| arg == "--reverse");
    poses(false, reverse);
    poses(true, reverse);
    scene(reverse)
}
