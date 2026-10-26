use super::model::*;

fn process(pid: u32, parent: u32, name: &str, birth: u64) -> Process {
    Process {
        pid,
        parent,
        name: name.into(),
        birth,
    }
}
fn sample(at: u64, percent: f64) -> Snapshot {
    Snapshot {
        unix_ms: at,
        processes: vec![
            process(1, 0, "ara-dev.exe", 1),
            process(2, 1, "ara-player.exe", 2),
            process(9, 0, "other.exe", 9),
        ],
        engines: vec![
            ("pid_2_luid_0_eng_0_engtype_3D".into(), 95.),
            ("pid_9_luid_0_eng_0_engtype_3D".into(), percent),
        ],
    }
}

#[test]
fn owned_players_are_excluded_and_single_bursts_do_not_invalidate() {
    let mut c = Classifier::new(1).unwrap();
    let mut r = Report::default();
    c.push(sample(1000, 6.), &mut r);
    c.push(sample(2000, 0.), &mut r);
    c.push(sample(3000, 6.), &mut r);
    assert!(!r.invalid());
    assert_eq!(r.samples[0].foreign_percent, 6.);
    assert!(
        r.samples
            .iter()
            .all(|s| s.foreign.iter().all(|p| p.pid != 2))
    );
}
#[test]
fn three_summed_samples_retain_original_start_peak_and_end() {
    let mut c = Classifier::with_policy(
        1,
        Policy {
            threshold_percent: 5.,
            sustained_samples: 3,
            ..Policy::default()
        },
    )
    .unwrap();
    let mut r = Report::default();
    for (at, load) in [(1000, 6.), (2000, 12.), (3000, 8.), (4000, 7.)] {
        let mut s = sample(at, load - 3.);
        s.engines
            .push(("pid_9_luid_0_eng_1_engtype_Copy".into(), 3.));
        c.push(s, &mut r);
    }
    assert!(r.invalid());
    let p = &r.contaminants[0];
    assert_eq!(
        (p.pid, p.first_unix_ms, p.last_unix_ms, p.peak_percent),
        (9, 0, 4000, 12.)
    );
    assert_eq!(r.status(), "invalid");
}
#[test]
fn actual_game_executables_block_even_between_gpu_bursts() {
    let mut c = Classifier::new(1).unwrap();
    let mut r = Report::default();
    c.push(
        Snapshot {
            unix_ms: 5,
            processes: vec![
                process(1, 0, "ara-dev.exe", 1),
                process(6, 0, "VaLoRaNt-Win64-Shipping.exe", 6),
                process(7, 0, "steam.exe", 7),
                process(8, 7, "steamwebhelper.exe", 8),
                process(9, 7, "a-game.exe", 9),
            ],
            engines: vec![],
        },
        &mut r,
    );
    assert!(r.invalid());
    assert_eq!(
        r.contaminants.iter().map(|p| p.pid).collect::<Vec<_>>(),
        vec![6]
    );
}

#[test]
fn idle_services_clients_and_overlays_never_invalidate_by_presence() {
    let names = [
        "vgc.exe",
        "vgtray.exe",
        "RiotClientServices.exe",
        "RiotClientUx.exe",
        "steam.exe",
        "steamwebhelper.exe",
        "EpicGamesLauncher.exe",
        "Discord.exe",
        "NVIDIA Overlay.exe",
        "GameOverlayUI.exe",
    ];
    let mut c = Classifier::new(1).unwrap();
    let mut report = Report::default();
    for at in 1..=3 {
        c.push(
            Snapshot {
                unix_ms: at,
                processes: names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| process(i as u32 + 10, 14, n, 1))
                    .collect(),
                engines: names
                    .iter()
                    .enumerate()
                    .map(|(i, _)| (format!("pid_{}_luid_0_eng_0", i + 10), 0.))
                    .collect(),
            },
            &mut report,
        );
    }
    assert!(!report.invalid());
    assert!(report.contaminants.is_empty());
    // Idle residents are not an exclusion list: actual GPU work still counts.
    for at in 4..=8 {
        c.push(
            Snapshot {
                unix_ms: at,
                processes: vec![process(99, 0, "NVIDIA Overlay.exe", 1)],
                engines: vec![("pid_99_luid_0_eng_0".into(), 11.)],
            },
            &mut report,
        );
    }
    assert!(report.invalid());
    assert_eq!(report.contaminants[0].pid, 99);
}

#[test]
fn compositor_allowance_and_kernel_information_do_not_hide_desktop_work() {
    let mut c = Classifier::new(1).unwrap();
    let mut r = Report::default();
    for at in 1..=5 {
        c.push(
            Snapshot {
                unix_ms: at * 1000,
                processes: vec![
                    process(4, 0, "System", 1),
                    process(20, 0, "csrss.exe", 1),
                    process(21, 0, "dwm.exe", 1),
                    process(22, 0, "Discord.exe", 1),
                ],
                engines: vec![
                    ("pid_4_eng_0".into(), 21.),
                    ("pid_20_eng_0".into(), 15.),
                    ("pid_21_eng_0".into(), 0.5),
                    ("pid_22_eng_0".into(), 9.),
                ],
            },
            &mut r,
        );
    }
    assert!(!r.invalid());
    assert_eq!(r.samples[0].foreign_percent, 9.);
    assert_eq!(r.samples[0].informational.len(), 3);
    for at in 6..=10 {
        let mut s = sample(at * 1000, 9.);
        s.processes.push(process(21, 0, "dwm.exe", 1));
        s.engines.push(("pid_21_eng_0".into(), 12.));
        c.push(s, &mut r);
    }
    assert!(r.invalid());
    assert_eq!(r.samples.last().unwrap().foreign_percent, 18.);
    // Replaying charged DWM values must never subtract the allowance twice.
    let replay = r.reclassify(Policy::default()).unwrap();
    assert_eq!(replay.samples.last().unwrap().foreign_percent, 18.);
    assert_eq!(r.intervals, replay.intervals);
    assert!(
        !r.reclassify(Policy {
            dwm_allowance_percent: 12.,
            ..Policy::default()
        })
        .unwrap()
        .invalid()
    );
}

#[test]
fn sustained_unknown_game_load_and_separated_bursts_keep_exact_intervals() {
    let mut c = Classifier::new(1).unwrap();
    let mut r = Report::default();
    for at in 1..=20 {
        c.push(
            sample(at * 1000, if at <= 5 || at >= 16 { 30. } else { 0. }),
            &mut r,
        );
    }
    assert_eq!(
        r.intervals,
        vec![
            Interval {
                start_unix_ms: 0,
                end_unix_ms: 5000
            },
            Interval {
                start_unix_ms: 15000,
                end_unix_ms: 20000
            }
        ]
    );
    assert!(!r.overlaps(Interval {
        start_unix_ms: 6000,
        end_unix_ms: 14000
    }));
    assert!(r.overlaps(Interval {
        start_unix_ms: 17000,
        end_unix_ms: 18000
    }));
    assert!(
        Policy {
            threshold_percent: f64::NAN,
            ..Policy::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Policy {
            sustained_samples: 0,
            ..Policy::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        !r.reclassify(Policy {
            threshold_percent: 31.,
            ..Policy::default()
        })
        .unwrap()
        .invalid()
    );
}

#[test]
fn recorded_desktop_run_invalidates_outpost_low_but_keeps_medium_clean() {
    #[derive(serde::Deserialize)]
    struct Fixture {
        processes: Vec<Load>,
        samples: Vec<(u64, Vec<(usize, f64)>)>,
        windows: std::collections::BTreeMap<String, Interval>,
    }
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/desktop-20261026.json")).unwrap();
    let mut c = Classifier::new(0).unwrap();
    let mut report = Report::default();
    for (unix_ms, values) in fixture.samples {
        let raw = values
            .into_iter()
            .map(|(index, percent)| Load {
                percent,
                ..fixture.processes[index].clone()
            })
            .collect();
        c.observe(
            Sample {
                unix_ms,
                interval_start_unix_ms: 0,
                foreign_percent: 0.,
                foreign: vec![],
                raw,
                informational: vec![],
            },
            &mut report,
        );
    }
    assert_eq!(report.samples.len(), 436);
    assert_eq!(report.intervals.len(), 4);
    assert!(report.overlaps(fixture.windows["outpost_low"]));
    assert!(!report.overlaps(fixture.windows["outpost_medium"]));
}
#[test]
fn orphaned_owned_children_stay_owned_but_recycled_pids_do_not() {
    let mut c = Classifier::new(1).unwrap();
    let mut r = Report::default();
    let mut initial = sample(1, 0.);
    initial.processes.push(process(3, 2, "child.exe", 3));
    c.push(initial, &mut r);
    for birth in [3, 30] {
        let s = Snapshot {
            unix_ms: birth,
            processes: vec![
                process(1, 0, "ara-dev.exe", 1),
                process(3, 2, "new.exe", birth),
            ],
            engines: vec![("pid_3_luid_0_eng_0".into(), 50.)],
        };
        let observed = c.push(s, &mut r);
        assert_eq!(observed.foreign_percent, if birth == 3 { 0. } else { 50. });
    }
}

#[test]
fn exited_owned_process_counter_tails_never_become_foreign() {
    let mut c = Classifier::new(1).unwrap();
    let mut r = Report::default();
    c.push(sample(1, 0.), &mut r);
    for at in 2..6 {
        let mut s = sample(at, 0.);
        s.processes.retain(|p| p.pid != 2);
        c.push(s, &mut r);
    }
    assert!(!r.invalid());
    assert!(r.samples.iter().all(|s| s.foreign_percent == 0.));
}
#[test]
fn bad_samples_and_monitor_errors_cannot_be_valid() {
    assert!(Report::default().reclassify(Policy::default()).is_err());
    let mut c = Classifier::new(1).unwrap();
    let mut r = Report::default();
    c.push(sample(1, f64::NAN), &mut r);
    assert!(r.invalid());
    r.errors.push("PDH unavailable".into());
    assert_eq!(r.status(), "invalid");
    assert_eq!(counter_pid("pid_392_luid_0"), Some(392));
    assert_eq!(counter_pid("not-a-pid"), None);
}

#[test]
fn cli_defaults_and_overrides_share_the_same_validated_policy() {
    use clap::Parser;
    let crate::Command::Gpu { gpu_policy, .. } =
        crate::Cli::try_parse_from(["ara-dev", "gpu", "--", "unused"])
            .unwrap()
            .command
    else {
        unreachable!()
    };
    assert_eq!(gpu_policy, Policy::default());
    assert!(
        crate::Cli::try_parse_from([
            "ara-dev",
            "d26",
            "--reclassify",
            "unused",
            "--dwm-allowance-percent",
            "4"
        ])
        .is_ok()
    );
    let crate::Command::Gpu { gpu_policy, .. } = crate::Cli::try_parse_from([
        "ara-dev",
        "gpu",
        "--foreign-gpu-percent",
        "20",
        "--foreign-gpu-samples",
        "8",
        "--dwm-allowance-percent",
        "4",
        "--",
        "unused",
    ])
    .unwrap()
    .command
    else {
        unreachable!()
    };
    assert_eq!(
        gpu_policy,
        Policy {
            threshold_percent: 20.,
            sustained_samples: 8,
            dwm_allowance_percent: 4.
        }
    );
    for (flag, value) in [
        ("--foreign-gpu-percent", "NaN"),
        ("--foreign-gpu-samples", "0"),
        ("--dwm-allowance-percent", "-1"),
    ] {
        assert!(
            crate::Cli::try_parse_from(["ara-dev", "gpu", flag, value, "--", "unused"]).is_err()
        );
    }
}
