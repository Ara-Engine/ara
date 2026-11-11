use super::*;

fn target() -> PlayProfileTarget {
    PlayProfileTarget {
        world: "play:3".into(),
        player: 2,
        source: "opaque-old-incarnation".into(),
        binding: 41,
    }
}

#[test]
fn requests_preserve_session_incarnation_and_all_real_actions() {
    for (ui, backend) in [
        (Action::Start, PlayProfileAction::Start),
        (Action::Stop, PlayProfileAction::Stop),
        (Action::Snapshot, PlayProfileAction::Snapshot),
    ] {
        let value = request_to_play(Request {
            session: "original-editor-session".into(),
            target: scope(target()),
            action: ui,
            why: Some("Inspect exact client frame history".into()),
        });
        assert_eq!(value.session, "original-editor-session");
        assert_eq!(value.target, target());
        assert_eq!(value.action, backend);
        assert_eq!(
            value.why.as_deref(),
            Some("Inspect exact client frame history")
        );
    }
}

#[test]
fn roster_retains_exited_incarnations_and_exact_pid_without_fallback() {
    let mut source = PlayProfileTargetRow {
        target: target(),
        name: "Client two · levels/test.scene".into(),
        process_id: 9227,
        recording: false,
        available: false,
        reason: Some("The selected player incarnation exited".into()),
    };
    let converted = row(&source);
    assert!(same_row(&converted, &source));
    assert_eq!(converted.target, scope(target()));
    assert_eq!(converted.process_id, Some(9227));
    assert!(!converted.available);
    assert_eq!(converted.reason, source.reason);
    source.target.source = "replacement-incarnation".into();
    source.target.binding += 1;
    assert!(!same_row(&converted, &source));
    assert_eq!(converted.target.source, "opaque-old-incarnation");
    source.target = target();
    source.process_id += 1;
    assert!(!same_row(&converted, &source));
}

#[test]
fn completed_status_moves_120_frames_and_published_diagnostics_without_reencoding() {
    let profile = ara_profile::Profile {
        version: 1,
        budget_ns: 16_666_667,
        labels: vec!["Player.Update".into()],
        threads: vec![ara_profile::ThreadInfo {
            id: 1,
            name: "Player main".into(),
        }],
        frames: (1..=120)
            .map(|id| ara_profile::Frame {
                id,
                duration_ns: 10_000_000,
                spans: vec![ara_profile::CpuSpan {
                    label: 0,
                    thread: 1,
                    id,
                    parent: 0,
                    start_ns: 0,
                    duration_ns: 4_000,
                    self_ns: 4_000,
                    allocated_bytes: 0,
                    self_allocated_bytes: 0,
                }],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let frames = profile.frames.as_slices().0.as_ptr();
    let spans = profile.frames[0].spans.as_ptr();
    let diagnostics = vec![ara_diag::Diagnostic::warning(ara_play_host::codes::PLAY, "GPU timestamps unavailable on this target")
        .data(serde_json::json!({"play":{"world":"play:3","player":2,"source":"opaque-old-incarnation","binding":41}}))];
    let diagnostic_rows = diagnostics.as_ptr();
    let value = status(PlayProfileStatus {
        ticket: PlayProfileTicket(771),
        target: target(),
        finished: true,
        outcome: Some(PlayProfileOutcome::Completed),
        recording: false,
        profile: Some(profile),
        diagnostics,
    });
    assert_eq!(value.outcome, Some(Outcome::Completed));
    let profile = value.profile.expect("completed profile");
    assert_eq!(profile.frames.len(), 120);
    assert_eq!(profile.frames.as_slices().0.as_ptr(), frames);
    assert_eq!(profile.frames[0].spans.as_ptr(), spans);
    assert!(
        profile.frames.iter().all(|frame| frame.gpu.is_empty()),
        "absent GPU data is preserved, never synthesized"
    );
    assert_eq!(value.diagnostics.as_ptr(), diagnostic_rows);
    assert_eq!(
        value.diagnostics[0].data.as_deref().expect("scope")["play"]["binding"],
        41
    );
}

#[test]
fn pending_refused_and_failed_statuses_preserve_their_original_tokens_and_scope() {
    for (backend, ui) in [
        (None, None),
        (
            Some(PlayProfileOutcome::Unchanged),
            Some(Outcome::Unchanged),
        ),
        (Some(PlayProfileOutcome::Failed), Some(Outcome::Failed)),
        (Some(PlayProfileOutcome::Refused), Some(Outcome::Refused)),
    ] {
        let value = status(PlayProfileStatus {
            ticket: PlayProfileTicket(113),
            target: target(),
            finished: backend.is_some(),
            outcome: backend,
            recording: false,
            profile: None,
            diagnostics: Vec::new(),
        });
        assert_eq!(value.finished, backend.is_some());
        assert_eq!(value.outcome, ui);
        assert!(value.profile.is_none());
    }
}
