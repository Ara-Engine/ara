//! CPU-only committed live-edit timing: setup, source dumps and hashes stay outside.
//! --rows 10000|100000 --case one|pct1|pct10|all|prefab-name|prefab-override --samples 5.
//! Compare the identical probe built against frozen original and candidate projection.
#[path = "projection_edit_bench/fixture.rs"]
mod fixture;
use ara_components::Transform;
use ara_doc::{AssetId, Workspace};
use ara_schema::SchemaDb;
use ara_sim::bevy_ecs::world::World;
use ara_sim::{EditWorld, Identity, Session, SimRegistry, StateHash, TickConfig};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Instant;

fn number(name: &str, default: usize) -> usize {
    std::env::args()
        .skip_while(|a| a != name)
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn prepare(schema: &SchemaDb, source: &Workspace) -> EditWorld {
    // A preexisting gameplay row is outside the authoring mirror, as in a prepared
    // runtime world. Both projection paths must leave its values and lifetime intact.
    let mut world = World::new();
    world.spawn((
        Identity("e_runtime".into()),
        Transform {
            position: [111.25, -0.0, 0.1],
            rotation: [17., 42., 5.],
            ..Default::default()
        },
    ));
    EditWorld::from_workspace_in(
        world,
        SimRegistry::new(schema.clone()).expect("registry"),
        source.clone(),
        AssetId::parse("sc_bench").expect("scene ID"),
    )
    .expect("projection")
}

fn hashes(workspace: &Workspace, schema: &SchemaDb) -> BTreeMap<String, String> {
    workspace
        .docs()
        .map(|(path, doc)| {
            (
                path.to_owned(),
                StateHash::of_bytes(ara_doc::write(doc, schema).as_bytes()).to_string(),
            )
        })
        .collect()
}

fn main() {
    let rows = number("--rows", 10000).max(2);
    let samples = number("--samples", 5).max(1);
    let case = std::env::args()
        .skip_while(|a| a != "--case")
        .nth(1)
        .unwrap_or_else(|| "one".into());
    let (schema, source, expected, ops) = fixture::fixture(rows, &case);
    let expected_hashes = hashes(&expected, &schema);
    let mut timings = Vec::with_capacity(samples);
    let mut last = None;
    for iteration in 0..=samples {
        drop(last.take());
        let mut edit = prepare(&schema, &source);
        let handles = edit
            .world()
            .iter_entities()
            .filter_map(|entity| {
                let id = &entity.get::<Identity>()?.0;
                (id != "e_runtime").then(|| (id.clone(), entity.id()))
            })
            .collect::<Vec<_>>();
        let start = Instant::now();
        edit.apply_ops(&ops).expect("committed live edits");
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if iteration != 0 {
            timings.push(ms);
        }
        for (id, entity) in &handles {
            assert_eq!(edit.entity(id), Some(*entity), "handle {id}");
        }
        assert_eq!(
            hashes(edit.workspace(), &schema),
            expected_hashes,
            "canonical source mirror"
        );
        let runtime = edit
            .world()
            .iter_entities()
            .find(|entity| {
                entity
                    .get::<Identity>()
                    .is_some_and(|id| id.0 == "e_runtime")
            })
            .expect("runtime row retained");
        let runtime = runtime
            .get::<Transform>()
            .expect("runtime component retained");
        assert_eq!(
            runtime.position.map(f32::to_bits),
            [111.25f32, -0.0, 0.1].map(f32::to_bits)
        );
        assert_eq!(runtime.rotation, [17., 42., 5.]);
        last = Some(edit);
    }
    let session = Session::local(
        last.expect("sample world"),
        TickConfig::default(),
        901,
        Default::default(),
    )
    .expect("session");
    let state_hash = session.state_hash().expect("all runtime component values");
    let portable = session
        .snapshot(StateHash::of_bytes(b"projection-edit-bench"))
        .expect("full snapshot")
        .to_bytes()
        .expect("portable snapshot");
    let mut sorted = timings.clone();
    sorted.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"case":case,"rows":rows,"source_scene_roots":source.doc("sc_bench").expect("scene").records().len(),
        "ops":ops.len(),"samples":samples,"warmup":1,"apply_ops_ms":timings,
        "p50_ms":sorted[samples/2],"p95_ms":sorted[(samples*95/100).min(samples-1)],
        "canonical_document_hashes":expected_hashes,"runtime_state_hash":state_hash.to_string(),
        "portable_snapshot_hash":StateHash::of_bytes(&portable).to_string(),
        "all_authored_handles_retained":true,"preexisting_runtime_values_retained":true})
    );
}
