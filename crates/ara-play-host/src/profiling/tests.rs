use super::*;

fn target() -> PlayProfileTarget {
    PlayProfileTarget {
        world: "play:1".into(),
        player: 0,
        source: "incarnation-one".into(),
        binding: 1,
    }
}
fn request() -> PlayProfileRequest {
    PlayProfileRequest {
        session: "editor".into(),
        target: target(),
        action: PlayProfileAction::Start,
        why: None,
    }
}

#[test]
fn unavailable_targets_are_refused_only_after_host_publication_and_unknown_tickets_stay_unknown() {
    let mut controller = Controller::default();
    let mut handle = controller.handle(Path::new("project"));
    assert!(handle.poll(PlayProfileTicket(999)).is_none());
    let ticket = handle.request(request()).expect("request");
    assert!(!handle.poll(ticket).expect("pending").finished);
    let mut published = Vec::new();
    controller.tick(
        &mut BTreeMap::new(),
        &mut crate::scripts::DiagnosticSources::default(),
        Path::new("project"),
        &mut |diagnostic| {
            assert!(
                !handle
                    .poll(ticket)
                    .expect("pending before publication")
                    .finished
            );
            published.push(diagnostic);
        },
    );
    let status = handle.poll(ticket).expect("terminal");
    assert!(status.finished);
    assert_eq!(status.outcome, Some(PlayProfileOutcome::Refused));
    assert_eq!(status.target, target());
    assert!(status.profile.is_none());
    assert_eq!(published.len(), 1);
    assert!(published[0].location.file.is_some());
}

#[test]
fn repeated_same_action_coalesces_but_start_stop_start_keeps_control_order() {
    let controller = Controller::default();
    let mut handle = controller.handle(Path::new("project"));
    let first = handle.request(request()).expect("start");
    assert_eq!(first, handle.request(request()).expect("coalesced"));
    let stop = handle
        .request(PlayProfileRequest {
            action: PlayProfileAction::Stop,
            ..request()
        })
        .expect("stop");
    let second = handle.request(request()).expect("new start after stop");
    assert_ne!(first, stop);
    assert_ne!(first, second);
    assert_ne!(stop, second);
}

#[test]
fn late_snapshot_cannot_reenable_recording_or_cross_a_player_incarnation() {
    let mut controller = Controller::default();
    controller.rows.push(PlayProfileTargetRow {
        target: target(),
        name: "Chosen".into(),
        process_id: 42,
        recording: false,
        available: true,
        reason: None,
    });
    let (reply, _receiver) = mpsc::sync_channel(4);
    controller.tasks.push(Task {
        status: Some(PlayProfileStatus {
            ticket: PlayProfileTicket(9),
            target: target(),
            finished: false,
            outcome: None,
            recording: true,
            profile: None,
            diagnostics: Vec::new(),
        }),
        reply,
        started: Instant::now(),
    });
    let make = |revision, ticket, recording| Reply {
        revision,
        ticket,
        recording,
        available: true,
        reason: None,
        outcome: Some(PlayProfileOutcome::Completed),
        profile: None,
        diagnostics: Vec::new(),
    };
    let mut published = Vec::new();
    let mut publish = |diagnostic| published.push(diagnostic);
    controller.accept(
        "play:1",
        0,
        "incarnation-one",
        make(6, 0, false),
        Path::new("project"),
        &mut publish,
    );
    controller.accept(
        "play:1",
        0,
        "other-process",
        make(7, 9, true),
        Path::new("project"),
        &mut publish,
    );
    assert!(
        !controller.tasks[0]
            .status
            .as_ref()
            .expect("pending task")
            .finished
    );
    let mut late = make(5, 9, true);
    late.diagnostics
        .push(Diagnostic::info(crate::codes::PLAY, "capture diagnostic"));
    controller.accept(
        "play:1",
        0,
        "incarnation-one",
        late,
        Path::new("project"),
        &mut publish,
    );
    let result = controller.tasks[0].status.as_ref().expect("finished task");
    assert!(result.finished);
    assert!(!result.recording);
    assert!(!controller.rows[0].recording);
    assert_eq!(published.len(), 1);
    let data = published[0].data.as_ref().expect("scoped diagnostic data");
    assert_eq!(data["play"]["source"], "incarnation-one");
    assert_eq!(data["play"]["binding"], 1);
}
