//! The actual direct hot kernels keep L1 body edits, borrowed native state and tails.
#[path = "../examples/ecs_kernel_bench/direct.rs"]
mod direct;
#[path = "../examples/ecs_kernel_bench/reference.rs"]
mod reference;
#[path = "../examples/ecs_kernel_bench/simd.rs"]
mod simd;
#[path = "../examples/ecs_kernel_bench/source.rs"]
mod source;
mod support;

use ara_script::abi::RELOAD_L1;
use bevy_ecs::entity::Entity;
use serde_json::json;

fn vector(live: &support::Live, entity: Entity, name: &str) -> [f32; 4] {
    let value = live
        .host
        .read_component(&live.world, entity, name)
        .expect("native component remains present");
    let fields = value["value"].as_array().expect("float4");
    assert_eq!(fields.len(), 4);
    std::array::from_fn(|axis| fields[axis].as_f64().expect("finite float4 field") as f32)
}

fn assert_state(live: &support::Live, entities: &[Entity], workload: &str, edited_ticks: usize) {
    for (id, &entity) in entities.iter().enumerate() {
        assert!(
            live.world.get_entity(entity).is_ok(),
            "native handle retained"
        );
        let stable_id = live
            .host
            .read_component(&live.world, entity, "BenchStableId")
            .expect("stable ID retained");
        assert_eq!(stable_id["value"].as_u64(), Some(id as u64));
        let mut expected = reference::initial(id as u32);
        for _ in 0..live.frame {
            if workload == "w1" {
                reference::transform(&mut expected);
            } else {
                reference::particle(&mut expected);
            }
        }
        // Eleven rows stay in one native run. The observable body-only edit changes
        // this row before either the native or scalar path; velocity.w is always zero.
        if id == 0 {
            expected[0][3] += edited_ticks as f32 * 0.25;
        }
        let actual = ["BenchPosition", "BenchVelocity", "BenchRotation"]
            .map(|name| vector(live, entity, name));
        for (field, (actual, expected)) in actual
            .into_iter()
            .flatten()
            .zip(expected.into_iter().flatten())
            .enumerate()
        {
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "{workload} row {id} field {field}"
            );
        }
    }
}

#[test]
fn actual_direct_hot_bodies_apply_l1_and_keep_all_native_fields_and_partial_tails() {
    let _clr = support::clr_lock();
    for workload in ["w1", "w3"] {
        eprintln!(
            "direct L1 workload={workload} compiled_backend={}",
            direct::backend()
        );
        let code = source::module(workload, "direct");
        let mut live = support::Live::start("direct-kernel-reload", &[("Game", &code, &[])]);
        let generation = live.host.modules()[0].generation;
        assert!(
            live.host.modules()[0].hot,
            "the edited module must remain EnC-enabled"
        );
        let entities = (0..11)
            .map(|id| {
                let [p, v, q] = reference::initial(id);
                live.spawn(&[
                    ("BenchStableId", json!({"value": id})),
                    ("BenchPosition", json!({"value": p})),
                    ("BenchVelocity", json!({"value": v})),
                    ("BenchRotation", json!({"value": q})),
                ])
            })
            .collect::<Vec<_>>();
        live.run();
        assert_state(&live, &entities, workload, 0);
        // This edits the real generated direct Run body, rather than a proxy or
        // helper. An L3 replacement would produce the value but fail the report gate.
        live.edit(
            "Game",
            "if (count == 0) return;",
            "if (count == 0) return;\n        positions[0].Value.W += 0.25f;",
        );
        live.until("the direct kernel's observable L1 body edit", |live| {
            vector(live, entities[0], "BenchPosition")[3] != 1.0
        });
        assert_eq!(
            live.last().kind,
            RELOAD_L1,
            "body edit must use a real EnC delta"
        );
        assert_eq!(live.count(RELOAD_L1), 1);
        assert_eq!(live.host.modules()[0].generation, generation);
        assert!(live.host.modules()[0].hot);
        assert_state(&live, &entities, workload, 1);
        for edited_ticks in 2..=4 {
            live.run();
            assert_state(&live, &entities, workload, edited_ticks);
        }
        assert!(
            live.diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.is_error()),
            "{:?}",
            live.diagnostics
        );
        assert_eq!(live.finish(), 0, "all module generations unload");
    }
}
