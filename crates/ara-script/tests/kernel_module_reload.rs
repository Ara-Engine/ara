//! Wrapper L1 keeps the kernel cold; editing the kernel heats it through an atomic L3 cascade.
#[path = "../examples/ecs_kernel_bench/direct.rs"]
#[allow(dead_code)] // The shared source selector also carries optional direct mode.
mod direct;
#[path = "../examples/ecs_kernel_bench/reference.rs"]
mod reference;
#[path = "../examples/ecs_kernel_bench/simd.rs"]
mod simd;
#[path = "../examples/ecs_kernel_bench/source.rs"]
mod source;
mod support;
#[path = "../examples/ecs_kernel_bench/topology.rs"]
mod topology;

use ara_script::abi::{ESC_COLD, ESC_RELINK, RELOAD_L1, RELOAD_L3};
use bevy_ecs::entity::Entity;
use serde_json::json;

fn vector(live: &support::Live, entity: Entity, name: &str) -> [f32; 4] {
    let component = live
        .host
        .read_component(&live.world, entity, name)
        .expect("native component retained");
    let values = component["value"].as_array().expect("full float4");
    assert_eq!(values.len(), 4);
    std::array::from_fn(|axis| values[axis].as_f64().expect("finite field") as f32)
}

fn assert_state(
    live: &support::Live,
    entities: &[Entity],
    workload: &str,
    wrapper_edited_ticks: usize,
    kernel_edited_ticks: usize,
) {
    let component_ids = [
        "BenchStableId",
        "BenchPosition",
        "BenchVelocity",
        "BenchRotation",
    ]
    .map(|name| {
        live.host
            .component_id(name)
            .expect("registered native column")
    });
    let mut observed = std::collections::HashSet::new();
    for archetype in live.world.archetypes().iter() {
        if !archetype.entities().is_empty()
            && component_ids.iter().any(|id| archetype.contains(*id))
        {
            assert!(component_ids.iter().all(|id| archetype.contains(*id)));
            for row in archetype.entities() {
                assert!(observed.insert(row.id()), "native identity occurs once");
            }
        }
    }
    assert_eq!(observed, entities.iter().copied().collect());
    for (id, &entity) in entities.iter().enumerate() {
        assert!(
            live.world.get_entity(entity).is_ok(),
            "native handle retained"
        );
        let stable_id = live
            .host
            .read_component(&live.world, entity, "BenchStableId")
            .expect("stable ID");
        assert_eq!(stable_id["value"].as_u64(), Some(id as u64));
        let mut expected = reference::initial(id as u32);
        for _ in 0..live.frame {
            if workload == "w1" {
                reference::transform(&mut expected);
            } else {
                reference::particle(&mut expected);
            }
        }
        // Eleven contiguous rows exercise two SIMD groups and all three scalar tails.
        // Both edits use exact binary fractions. The original kernel preserves p.w
        // because v.w=0; row zero also proves the wrapper's L1 survives its relink.
        if id == 0 {
            expected[0][3] += wrapper_edited_ticks as f32 * 0.25;
        }
        expected[0][3] += kernel_edited_ticks as f32 * 0.125;
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
                "{workload} ID {id} field {field}"
            );
        }
    }
}

#[test]
fn hot_wrapper_l1_then_cold_kernel_l3_preserve_native_columns_and_three_tails() {
    let _clr = support::clr_lock();
    for workload in ["w1", "w3"] {
        let mut live = topology::start(workload, "simd", true, true).expect("two modules compile");
        let mut modules = live.host.modules();
        modules.sort_by(|left, right| left.name.cmp(&right.name));
        eprintln!("kernel topology L1 workload={workload} before={modules:?}");
        let game = modules
            .iter()
            .find(|module| module.name == "Game")
            .expect("Game module");
        let kernel = modules
            .iter()
            .find(|module| module.name == "Kernel")
            .expect("Kernel module");
        assert!(game.hot, "the actual edited wrapper must stay EnC-enabled");
        assert!(!kernel.hot, "the actual math module must start cold");
        let game_slot = game.slot;
        let kernel_source = std::fs::read(live.file("Kernel")).expect("cold kernel source");
        let ids = [
            "BenchStableId",
            "BenchPosition",
            "BenchVelocity",
            "BenchRotation",
        ]
        .map(|name| live.host.component_id(name).expect("native component ID"));
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
        // Initial module-load reports are retained until the first boundary.
        live.tick();
        assert_state(&live, &entities, workload, 0, 0);
        let reports_before_edit = live.reports.len();
        live.edit(
            "Game",
            "        Kernels.Batch(",
            "        positions[0].Value.W += 0.25f;\n        Kernels.Batch(",
        );
        live.until("the hot wrapper's observable body edit", |live| {
            vector(live, entities[0], "BenchPosition")[3] != 1.0
        });
        assert_eq!(
            (live.last().module, live.last().kind, live.last().cascade),
            (game_slot, RELOAD_L1, 1)
        );
        assert_eq!(
            live.reports.len() - reports_before_edit,
            1,
            "only the wrapper applies one EnC delta after the edit"
        );
        assert_state(&live, &entities, workload, 1, 0);
        for edited_ticks in 2..=4 {
            live.run();
            assert_state(&live, &entities, workload, edited_ticks, 0);
        }
        let mut after = live.host.modules();
        after.sort_by(|left, right| left.name.cmp(&right.name));
        eprintln!("kernel topology L1 workload={workload} after={after:?}");
        assert_eq!(
            after, modules,
            "both actual heats, generations, slots and system counts stay fixed"
        );
        assert_eq!(
            std::fs::read(live.file("Kernel")).expect("cold kernel source"),
            kernel_source
        );
        assert_eq!(
            [
                "BenchStableId",
                "BenchPosition",
                "BenchVelocity",
                "BenchRotation"
            ]
            .map(|name| live
                .host
                .component_id(name)
                .expect("native component retained")),
            ids
        );

        // Change the real cold arithmetic body without changing its public surface.
        // R10 heats this module at L3; Game must relink with its existing L1 intact.
        let entry = std::str::from_utf8(&kernel_source)
            .expect("kernel source is UTF-8")
            .lines()
            .find(|line| line.contains("public static void Batch("))
            .expect("the actual borrowed-span batch entry");
        let before_body = format!("{entry}\n    {{");
        let after_body = format!(
            "{before_body}\n        for (var row = 0; row < positions.Length; row++) positions[row].Value.W += 0.125f;"
        );
        let frame_before_kernel_edit = live.frame;
        let reports_before_kernel_edit = live.reports.len();
        let game_source = std::fs::read(live.file("Game")).expect("edited Game source");
        let mut expected_heated = modules.clone();
        for module in &mut expected_heated {
            module.hot = true;
            module.generation += 1;
        }
        live.edit("Kernel", &before_body, &after_body);
        live.until(
            "the cold kernel's observable arithmetic body edit",
            |live| {
                let reports = &live.reports[reports_before_kernel_edit..];
                assert!(
                    reports.is_empty() || reports.len() == 2,
                    "both cascade reports must arrive at the same boundary: {reports:?}"
                );
                let mut actual = live.host.modules();
                actual.sort_by(|left, right| left.name.cmp(&right.name));
                assert_eq!(
                    actual.as_slice(),
                    if reports.is_empty() {
                        modules.as_slice()
                    } else {
                        expected_heated.as_slice()
                    },
                    "both module generations and heats transition at that boundary"
                );
                vector(live, entities[1], "BenchPosition")[3] != 1.0
            },
        );
        let cascade = live.reports[reports_before_kernel_edit..]
            .iter()
            .map(|report| {
                (
                    report.module,
                    report.kind,
                    report.escalation,
                    report.cascade,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            cascade,
            [
                (kernel.slot, RELOAD_L3, ESC_COLD, 2),
                (game_slot, RELOAD_L3, ESC_RELINK, 2),
            ],
            "exactly the cold rebuild and dependent relink must apply together"
        );
        let mut heated = live.host.modules();
        heated.sort_by(|left, right| left.name.cmp(&right.name));
        eprintln!("kernel topology L3 workload={workload} after={heated:?} reports={cascade:?}");
        assert_eq!(
            heated, expected_heated,
            "the edited kernel heats; both generations advance once"
        );
        for kernel_edited_ticks in 1..=4 {
            if kernel_edited_ticks > 1 {
                live.run();
            }
            let wrapper_edited_ticks = 4 + (live.frame - frame_before_kernel_edit) as usize;
            assert_state(
                &live,
                &entities,
                workload,
                wrapper_edited_ticks,
                kernel_edited_ticks,
            );
        }
        assert_eq!(
            std::fs::read(live.file("Game")).expect("relinked Game source"),
            game_source
        );
        assert_eq!(
            [
                "BenchStableId",
                "BenchPosition",
                "BenchVelocity",
                "BenchRotation"
            ]
            .map(|name| live
                .host
                .component_id(name)
                .expect("native component retained after L3")),
            ids,
            "unchanged layouts retain native component IDs through the cascade"
        );
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
