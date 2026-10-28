//! Controlled private-port observations test UI identity/ownership, not player transport acceptance.
#![allow(clippy::unwrap_used)]
mod fixture;
use super::*;
use ara_ui::{input::InputEvent, source::stable_id};
use fixture::*;

#[test]
fn absent_player_transport_leaves_real_editor_recording_functional() {
    let mut f = Fixture::new(false);
    let revision = f.editor.host.store().revision();
    assert!(!f.shell.poll_profiler_target_state(&mut f.editor).unwrap());
    assert!(f.shell.profiler_target_blocker().is_none());
    f.shell.profiler_record_target(&mut f.editor).unwrap();
    assert!(f.editor.host.profiler().is_recording());
    f.shell.profiler_record_target(&mut f.editor).unwrap();
    assert!(!f.editor.host.profiler().is_recording());
    assert_eq!(f.editor.host.store().revision(), revision);
    assert!(!f.shell.profiler_targets_busy());
}

#[test]
fn missing_incarnation_is_not_replaced_and_completed_profile_moves_once() {
    let mut state = State {
        selected: Target::Player(scope(1, "old", 1)),
        transport: true,
        ..State::default()
    };
    let old = scope(1, "old", 1);
    state.observe_rows(&[row(1, "replacement", 2)]);
    assert_eq!(state.selected, Target::Player(old.clone()));
    assert!(state.blocker().unwrap().contains("no longer live"));
    state.pending = Some(Pending {
        ticket: Ticket(1),
        target: old.clone(),
        purpose: Purpose::Snapshot,
        session: "original-session".into(),
    });
    let profile = profile();
    let spans = profile.frames[0].spans.as_ptr();
    assert!(state.accept(Status {
        ticket: Ticket(1),
        target: old.clone(),
        finished: true,
        outcome: Some(Outcome::Completed),
        recording: false,
        profile: Some(profile),
        diagnostics: vec![]
    }));
    let capture = state.current.as_ref().unwrap();
    assert_eq!(capture.target, Target::Player(old));
    assert_eq!(
        capture.profile.frames[0].spans.as_ptr(),
        spans,
        "owned profile buffers are moved, not cloned"
    );
    assert_eq!(Arc::strong_count(&capture.profile), 1);
    assert!(state.blocker().unwrap().contains("no longer live"));
    assert!(state.pending.is_none());
}

#[test]
fn native_target_select_pending_typing_and_late_frame_actions_preserve_target_scope() {
    let mut f = Fixture::new(true);
    f.poll();
    f.shell.rebuild(&mut f.editor).unwrap();
    f.shell.frame().unwrap();
    click(&mut f.shell, &mut f.editor, "profile-target");
    f.shell.key(&mut f.editor, 77, true, &mut Clip).unwrap();
    f.shell.key(&mut f.editor, 40, true, &mut Clip).unwrap();
    let (ticket, request) = f.requests.as_ref().unwrap().try_recv().unwrap();
    assert_eq!(request.target, scope(2, "second", 1));
    assert_eq!(request.action, Action::Snapshot);
    assert_eq!(request.session, f.editor.session);
    assert!(f.shell.profiler_targets_busy());
    let revision = f.editor.host.store().revision();
    assert!(
        f.shell
            .input
            .focus_if_available(&mut f.shell.ui, stable_id("search-hierarchy"))
    );
    f.shell.native_text("λ", &mut Clip).unwrap();
    f.shell.dispatch(&mut f.editor).unwrap();
    for _ in 0..4 {
        f.editor.tick();
        f.poll();
    }
    assert!(
        f.requests.as_ref().unwrap().try_recv().is_err(),
        "one pending request stays single-flight while typing"
    );
    assert_eq!(f.editor.host.store().revision(), revision);
    f.shell
        .choose_profiler_target(&mut f.editor, Value::Null)
        .unwrap();
    f.shell.profiler_record_target(&mut f.editor).unwrap();
    assert!(
        f.editor.host.profiler().is_recording(),
        "pending player work does not block Editor recording"
    );
    f.shell.profiler_record_target(&mut f.editor).unwrap();
    let profile = profile();
    let spans = profile.frames[0].spans.as_ptr();
    f.reply(Status {
        ticket,
        target: request.target.clone(),
        finished: true,
        outcome: Some(Outcome::Completed),
        recording: false,
        profile: Some(profile),
        diagnostics: vec![],
    });
    assert_eq!(
        f.shell
            .profiler_targets
            .current
            .as_ref()
            .unwrap()
            .profile
            .frames[0]
            .spans
            .as_ptr(),
        spans
    );
    assert!(
        f.shell.profiler_reading(&mut f.editor).editor,
        "late player data never substitutes for Editor data"
    );
    f.shell
        .choose_profiler_target(&mut f.editor, json!(request.target))
        .unwrap();
    f.shell.frame().unwrap();
    let id = f.shell.profiler_frame_id(7);
    let Binding::Command(_, late) = f.shell.bindings[&stable_id(&id)].clone() else {
        panic!("scoped frame command")
    };
    f.shell
        .choose_profiler_target(&mut f.editor, json!(scope(1, "first", 1)))
        .unwrap();
    assert!(f.shell.choose_profiler_frame(&mut f.editor, &late).is_err());
    assert!(f.shell.profiler_targets.frame.is_none());
    let (ticket, request) = f.requests.as_ref().unwrap().try_recv().unwrap();
    f.reply(Status {
        ticket,
        target: request.target,
        finished: true,
        outcome: Some(Outcome::Refused),
        recording: false,
        profile: None,
        diagnostics: vec![ara_diag::Diagnostic::error(
            ara_play_host::codes::REQUEST,
            "Controlled refusal: target cannot provide this snapshot",
        )],
    });
    f.poll();
    assert!(
        f.shell
            .profiler_targets
            .error
            .as_deref()
            .unwrap()
            .contains("refused")
    );
    assert!(f.shell.ui.find("profile-target-error").is_some());
    assert!(!f.shell.profiler_targets_busy());
    assert_eq!(f.editor.host.store().revision(), revision);
}

#[test]
fn comparison_labels_both_incarnations_and_unknown_target_actions_stay_disabled() {
    let mut f = Fixture::new(true);
    f.poll();
    let current = scope(2, "second", 1);
    let baseline = scope(1, "first", 1);
    f.shell.profiler_targets.selected = Target::Player(current.clone());
    f.shell.profiler_targets.follow = false;
    f.shell.profiler_targets.current = Some(Capture {
        target: Target::Player(current),
        name: "Client two".into(),
        profile: Arc::new(profile()),
        serial: 2,
        frozen: true,
    });
    f.shell.profiler_targets.baseline = Some(Capture {
        target: Target::Player(baseline),
        name: "Client one".into(),
        profile: Arc::new(profile()),
        serial: 1,
        frozen: true,
    });
    let reading = f.shell.profiler_reading(&mut f.editor);
    assert!(reading.current_label.contains("second"));
    assert!(reading.baseline_label.contains("first"));
    assert!(!reading.compare);
    f.shell.compare_profiler_targets(&mut f.editor).unwrap();
    assert!(f.shell.profiler_reading(&mut f.editor).compare);
    f.rows(vec![row(1, "first", 2), row(2, "second", 2)]);
    f.shell.rebuild(&mut f.editor).unwrap();
    f.shell.frame().unwrap();
    assert_eq!(
        f.shell.profiler_targets.selected,
        Target::Player(scope(2, "second", 1))
    );
    assert!(
        f.shell
            .profiler_target_blocker()
            .unwrap()
            .contains("no longer live")
    );
    let record = f.shell.ui.find("profile-record").unwrap();
    assert!(f.shell.ui.nodes[record].state.disabled);
    assert!(
        f.shell.ui.nodes[f.shell.ui.find("profile-current-scope").unwrap()]
            .element
            .attrs["text"]
            .contains("binding 1")
    );
    let pointer = f
        .shell
        .profiler_targets
        .current
        .as_ref()
        .unwrap()
        .profile
        .frames[0]
        .spans
        .as_ptr();
    let reads = f
        .reads
        .as_ref()
        .unwrap()
        .load(std::sync::atomic::Ordering::Relaxed);
    f.shell.profiler_targets.next_poll = Some(Instant::now() + Duration::from_secs(3600));
    for _ in 0..128 {
        f.shell.poll_profiler_target_state(&mut f.editor).unwrap();
    }
    assert_eq!(
        f.reads
            .as_ref()
            .unwrap()
            .load(std::sync::atomic::Ordering::Relaxed),
        reads
    );
    assert_eq!(
        f.shell
            .profiler_targets
            .current
            .as_ref()
            .unwrap()
            .profile
            .frames[0]
            .spans
            .as_ptr(),
        pointer
    );
}

#[test]
fn another_ticket_keeps_the_original_profile_request_owned_and_pending() {
    let original = scope(1, "original-incarnation", 7);
    let mut state = State {
        selected: Target::Editor,
        pending: Some(Pending {
            ticket: Ticket(18), target: original.clone(), purpose: Purpose::Snapshot,
            session: "original-session".into(),
        }),
        ..State::default()
    };
    assert!(state.accept(Status {
        ticket: Ticket(19), target: original.clone(), finished: true,
        outcome: Some(Outcome::Completed), recording: false, profile: Some(profile()),
        diagnostics: Vec::new(),
    }));
    let pending = state.pending.as_ref().expect("original ownership retained");
    assert_eq!(pending.ticket, Ticket(18));
    assert_eq!(pending.target, original);
    assert_eq!(pending.session, "original-session");
    assert!(state.current.is_none());
    assert!(state.selected == Target::Editor, "late player data never substitutes for the selected Editor target");
    assert!(state.accept(Status {
        ticket: Ticket(18), target: original, finished: true,
        outcome: Some(Outcome::Refused), recording: false, profile: None,
        diagnostics: Vec::new(),
    }));
    assert!(state.pending.is_none());
    assert!(state.selected == Target::Editor);
}

#[test]
fn matching_ticket_from_another_incarnation_never_adopts_the_profile() {
    let original = scope(2, "original-incarnation", 7);
    let mut state = State {
        selected: Target::Player(original.clone()),
        pending: Some(Pending {
            ticket: Ticket(18), target: original.clone(), purpose: Purpose::Snapshot,
            session: "original-session".into(),
        }),
        ..State::default()
    };
    assert!(state.accept(Status {
        ticket: Ticket(18), target: scope(2, "replacement-incarnation", 8), finished: true,
        outcome: Some(Outcome::Completed), recording: false, profile: Some(profile()),
        diagnostics: Vec::new(),
    }));
    assert!(state.pending.is_none());
    assert!(state.current.is_none());
    assert!(state.selected == Target::Player(original));
    assert!(state.error.as_deref().expect("scope mismatch").contains("did not match"));
}
