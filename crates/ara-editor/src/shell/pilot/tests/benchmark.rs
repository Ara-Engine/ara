//! Explicit opt-in CPU cost of disabled polling versus an unchanged active pilot.
use super::*;
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "opt-in CPU benchmark; writes canonical metadata under the test target"]
fn benchmark_idle_pilot_polling() {
    let project = Project::new();
    let (mut editor, mut shell) = open(&project);
    shell.toggle_pilot(&mut editor, &Target::None).unwrap();
    let mut active = std::mem::take(&mut shell.pilot);
    let mut disabled = Pilot::default();
    let mut lookup = |_: &Editor| -> ara_ui::Result<Lookup> {
        panic!("unchanged idle polling must not resolve or clone the Scene")
    };
    for _ in 0..1000 {
        black_box(active.poll(&mut editor, false, false, &mut lookup).unwrap());
        black_box(
            disabled
                .poll(&mut editor, false, false, &mut lookup)
                .unwrap(),
        );
    }
    let count = 100_000;
    let mut before = Vec::new();
    let mut after = Vec::new();
    for round in 0..5 {
        for enabled in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let pilot = if enabled { &mut active } else { &mut disabled };
            let start = Instant::now();
            for _ in 0..count {
                black_box(
                    pilot
                        .poll(black_box(&mut editor), false, false, &mut lookup)
                        .unwrap(),
                );
            }
            let ns = start.elapsed().as_nanos() as f64 / f64::from(count);
            if enabled {
                after.push(ns);
            } else {
                before.push(ns);
            }
        }
    }
    before.sort_by(f64::total_cmp);
    after.sort_by(f64::total_cmp);
    let mut source = blake3::Hasher::new();
    source.update(include_bytes!("../../pilot.rs"));
    source.update(include_bytes!("../model.rs"));
    source.update(include_bytes!("../../../app/controller.rs"));
    let result = json!({"schema":1,"kind":"cpu-opt-in","measurement":"disabled versus active unchanged Pilot::poll; not a pre-feature source comparison","samplesPerRound":count,"interleavedRounds":5,"warmupPerArm":1000,"disabledMedianNsPerCall":before[2],"activeMedianNsPerCall":after[2],"disabledRoundsNs":before,"activeRoundsNs":after,"sceneLookups":0,"sourceBlake3":source.finalize().to_hex().to_string(),"nativeOrGpuEvidence":false});
    let output = crate::test_target().join("pilot-benchmarks");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(
        output.join(format!("idle-{}.json", std::process::id())),
        ara_json::to_canonical_string(&result),
    )
    .unwrap();
    eprintln!("{}", ara_json::to_canonical_string(&result));
}
