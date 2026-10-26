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
    let mut c = Classifier::new(1).unwrap();
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
        (9, 1000, 4000, 12.)
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
    for at in 4..=6 {
        c.push(
            Snapshot {
                unix_ms: at,
                processes: vec![process(99, 0, "NVIDIA Overlay.exe", 1)],
                engines: vec![("pid_99_luid_0_eng_0".into(), 6.)],
            },
            &mut report,
        );
    }
    assert!(report.invalid());
    assert_eq!(report.contaminants[0].pid, 99);
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
    let mut c = Classifier::new(1).unwrap();
    let mut r = Report::default();
    c.push(sample(1, f64::NAN), &mut r);
    assert!(r.invalid());
    r.errors.push("PDH unavailable".into());
    assert_eq!(r.status(), "invalid");
    assert_eq!(counter_pid("pid_392_luid_0"), Some(392));
    assert_eq!(counter_pid("not-a-pid"), None);
}
