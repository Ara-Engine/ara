//! Lifecycle state is exercised without starting a native player or GPU device.
use super::*;

fn seed(shell: &mut Shell, world: &str) {
    let target = FieldTarget {
        path: "Camera.fov".into(),
        pointer: String::new(),
        ty: ValueType::F32,
        unit: None,
    };
    shell.play_controls.runtime = Some(Runtime {
        world: world.into(),
        player: 0,
        entity: "e_playlayout".into(),
        tick: 4,
        components: json!({"Camera":{"fov":55}}),
        editable: true,
        drafts: BTreeMap::from([(
            target.key(),
            Draft {
                target,
                input: json!(65),
            },
        )]),
    });
    shell.play_controls.keep = Some(
        keep::Choice::read(
            world.into(),
            vec!["e_playlayout".into()],
            false,
            &json!({"keepCandidates":[]}),
        )
        .unwrap(),
    );
    shell.play_controls.spectators.insert(0);
    shell.play_controls.expanded.insert("Camera.fov".into());
    shell.play_controls.pages.insert("Camera.fov".into(), 2);
    shell.play_controls.message = Some("Previous runtime".into());
    shell.play_controls.last_keep = Some(json!({"kept":1}));
    shell.play_controls.players.insert(
        2,
        PlayerInfo {
            live: true,
            size: [640, 360],
            fps: 60,
        },
    );
    shell.play_controls.options = true;
    shell.game_players = vec![0, 1, 2];
    shell.game_client = 2;
    shell.game_running = true;
    shell.game_live = true;
    shell.game_captured = true;
}

fn cleared(shell: &Shell, world: Option<&str>) {
    assert_eq!(shell.play_controls.world.as_deref(), world);
    assert!(shell.play_controls.runtime.is_none());
    assert!(shell.play_controls.keep.is_none());
    assert!(shell.play_controls.spectators.is_empty());
    assert!(shell.play_controls.expanded.is_empty());
    assert!(shell.play_controls.pages.is_empty());
    assert!(shell.play_controls.message.is_none());
    assert!(shell.play_controls.last_keep.is_none());
    assert!(shell.play_controls.pending.is_none());
    assert!(shell.play_controls.players.is_empty());
    assert!(shell.game_players.is_empty());
    assert_eq!(shell.game_client, 0);
    assert!(!shell.game_running && !shell.game_live && !shell.game_captured);
    assert!(
        shell.play_controls.options,
        "The options drawer preference survives Stop/restart"
    );
}

#[test]
fn idle_stop_and_new_world_clear_inspection_drafts_keep_and_spectator_state() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor.play_world = Some("play:old".into());
    assert!(shell.poll_play_job(&mut editor).unwrap());
    seed(&mut shell, "play:old");
    let authority = editor.host.store().revision();
    editor.play_world = None;
    assert!(
        shell.poll_play_job(&mut editor).unwrap(),
        "An idle Stop must still reconcile its owner"
    );
    cleared(&shell, None);
    let presentation = shell.play_controls.revision;
    assert!(!shell.poll_play_job(&mut editor).unwrap());
    assert_eq!(shell.play_controls.revision, presentation);
    editor.play_world = Some("play:new".into());
    assert!(shell.poll_play_job(&mut editor).unwrap());
    cleared(&shell, Some("play:new"));
    assert_eq!(editor.host.store().revision(), authority);
}

#[test]
fn restart_reusing_the_world_id_invalidates_all_old_runtime_presentation() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor.play_world = Some("play:same".into());
    shell.poll_play_job(&mut editor).unwrap();
    seed(&mut shell, "play:same");
    shell.queue_simulate(&mut editor, true).unwrap();
    let authority = editor.host.store().revision();
    shell.reset_play_transients(&mut editor);
    cleared(&shell, Some("play:same"));
    assert!(shell.play_controls.simulate.is_none());
    assert!(!shell.poll_play_job(&mut editor).unwrap());
    assert_eq!(editor.host.store().revision(), authority);
}

#[test]
fn cancelled_before_ready_simulate_cannot_eject_the_next_plain_play() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor.play_world = Some("play:cancelled".into());
    shell.queue_simulate(&mut editor, true).unwrap();
    assert!(!shell.poll_play_job(&mut editor).unwrap());
    assert!(shell.play_controls.simulate.is_some());
    editor.play_world = None;
    assert!(shell.poll_play_job(&mut editor).unwrap());
    assert!(shell.play_controls.simulate.is_none());
    editor.play_world = Some("play:plain".into());
    shell.game_running = true; // Stale readiness must not dispatch the old request.
    let authority = editor.host.store().revision();
    assert!(shell.poll_play_job(&mut editor).unwrap());
    assert!(shell.play_controls.pending.is_none());
    assert!(
        shell
            .play_controls
            .take_simulate(Some("play:plain"), true)
            .is_none()
    );
    assert_eq!(editor.host.store().revision(), authority);
}

#[test]
fn fresh_simulate_survives_world_adoption_and_is_consumed_once_when_ready() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor.play_world = Some("play:first".into());
    shell.poll_play_job(&mut editor).unwrap();
    seed(&mut shell, "play:first");
    // A direct switch before the next poll still binds the fresh request to the
    // new startup, rather than retaining the old inspection or clearing intent.
    editor.play_world = Some("play:simulate".into());
    shell.queue_simulate(&mut editor, false).unwrap();
    cleared(&shell, Some("play:simulate"));
    assert!(!shell.poll_play_job(&mut editor).unwrap());
    assert!(!shell.poll_play_job(&mut editor).unwrap());
    assert!(
        shell
            .play_controls
            .take_simulate(Some("play:simulate"), false)
            .is_none()
    );
    assert_eq!(
        shell
            .play_controls
            .take_simulate(Some("play:simulate"), true),
        Some(false)
    );
    assert!(
        shell
            .play_controls
            .take_simulate(Some("play:simulate"), true)
            .is_none()
    );
    editor.play_world = None;
    assert!(shell.queue_simulate(&mut editor, true).is_err());
    assert!(shell.play_controls.simulate.is_none());
}

/// Previous pending=None branches, before adding per-world reconciliation.
#[inline(never)]
fn previous_idle_poll(shell: &mut Shell) -> ara_ui::Result<bool> {
    if shell.play_controls.pending.is_none()
        && shell.game_running
        && shell.play_controls.simulate.is_some()
    {
        return Err("The idle benchmark cannot contain a queued Simulate".into());
    }
    let Some(_) = shell.play_controls.pending.take() else {
        return Ok(false);
    };
    Err("The idle benchmark cannot contain a pending job".into())
}

#[test]
#[ignore = "Opt-in CPU measurement; writes a canonical target artifact"]
fn benchmark_idle_play_world_reconciliation() {
    use std::{hint::black_box, time::Instant};
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    editor.play_world = Some("play:idle".into());
    shell.poll_play_job(&mut editor).unwrap();
    let authority = editor.host.store().revision();
    let presentation = shell.play_controls.revision;
    let measure = |baseline: bool, shell: &mut Shell, editor: &mut Editor, count: usize| {
        let began = Instant::now();
        for _ in 0..count {
            let result = if baseline {
                previous_idle_poll(black_box(&mut *shell))
            } else {
                black_box(&mut *shell).poll_play_job(black_box(&mut *editor))
            };
            black_box(result.unwrap());
        }
        began.elapsed().as_nanos() as f64 / count as f64
    };
    measure(true, &mut shell, &mut editor, 1000);
    measure(false, &mut shell, &mut editor, 1000);
    let mut before = Vec::new();
    let mut after = Vec::new();
    for round in 0..5 {
        for baseline in if round % 2 == 0 {
            [true, false]
        } else {
            [false, true]
        } {
            let elapsed = measure(baseline, &mut shell, &mut editor, 10_000);
            if baseline {
                before.push(elapsed);
            } else {
                after.push(elapsed);
            }
        }
    }
    assert_eq!(editor.host.store().revision(), authority);
    assert_eq!(shell.play_controls.revision, presentation);
    let median = |samples: &[f64]| {
        let mut samples = samples.to_vec();
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    let output = crate::test_target().join("editor-play-lifecycle-benchmarks");
    std::fs::create_dir_all(&output).unwrap();
    let fingerprint = blake3::hash(
        concat!(include_str!("../lifecycle.rs"), include_str!("../jobs.rs")).as_bytes(),
    )
    .to_hex()
    .to_string();
    std::fs::write(output.join(format!("idle-poll-{}.json", std::process::id())), ara_json::to_canonical_string(&json!({"schema":1,"baseline":"Previous idle pending=None poll branches; test-only","current":"Real poll with unchanged owning world","productionSourceBlake3":fingerprint,"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"debugAssertions":cfg!(debug_assertions),"iterationsPerRound":10000,"rounds":5,"beforeNsPerCall":before,"afterNsPerCall":after,"beforeMedianNsPerCall":median(&before),"afterMedianNsPerCall":median(&after)}))).unwrap();
}
