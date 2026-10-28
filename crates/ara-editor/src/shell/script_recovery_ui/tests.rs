//! Controlled owned-channel completions; these tests neither start players nor emit diagnostics.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_play_host::ScriptRecoveryPlayerResult;

fn request(world: &str) -> ScriptRecoveryRequest {
    ScriptRecoveryRequest {
        session: "editor-session".into(),
        world: world.into(),
        player: None,
        why: Some("Editor: Reload Scripts".into()),
    }
}
fn status(ticket: u64, world: &str, finished: bool) -> ScriptRecoveryStatus {
    ScriptRecoveryStatus {
        ticket: ScriptRecoveryTicket(ticket),
        world: world.into(),
        finished,
        players: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn coalesces_only_same_world_and_keeps_previous_world_ticket_owned() {
    let mut state = State::default();
    assert!(state.start(request("play:one"), |_| Ok(ScriptRecoveryTicket(1))));
    assert!(!state.start(request("play:one"), |_| panic!(
        "same-world duplicate must not submit"
    )));
    assert!(state.start(request("play:two"), |_| Ok(ScriptRecoveryTicket(2))));
    assert_eq!(state.pending.len(), 2);
    assert!(state.note(Some("play:one")).unwrap().contains("play:one"));
    assert!(state.note(None).unwrap().contains("previous world"));
    let now = Instant::now();
    assert!(state.poll(now, |ticket| {
        (ticket.0 == 1).then(|| status(1, "play:one", true))
    }));
    assert!(!state.pending_world("play:one"));
    assert!(state.pending_world("play:two"));
    assert_eq!(state.terminal.front().unwrap().world, "play:one");
}

#[test]
fn mismatch_and_missing_reply_cannot_release_or_retarget_a_request() {
    let mut state = State::default();
    state.start(request("play:one"), |_| Ok(ScriptRecoveryTicket(1)));
    let now = Instant::now();
    assert!(!state.poll(now, |_| None));
    assert!(state.busy());
    assert!(
        state.poll(now + Duration::from_millis(250), |_| Some(status(
            1,
            "play:replacement",
            true
        )))
    );
    assert!(state.pending[&1].invalid_reply);
    assert!(state.busy());
    assert!(
        !state.poll(now + Duration::from_millis(500), |_| Some(status(
            99, "play:one", true
        )))
    );
    assert!(state.busy());
    assert!(
        state.poll(now + Duration::from_millis(750), |_| Some(status(
            1, "play:one", true
        )))
    );
    assert!(!state.busy());
    assert_eq!(state.terminal.front().unwrap().world, "play:one");
}

#[test]
fn slow_recovery_polls_are_bounded_coalesced_and_round_robin_without_releasing_tickets() {
    let mut state = State::default();
    for ticket in 1..=12 {
        state.start(request(&format!("play:{ticket}")), |_| {
            Ok(ScriptRecoveryTicket(ticket))
        });
    }
    let now = Instant::now();
    let mut read = Vec::new();
    assert!(!state.poll(now, |ticket| {
        read.push(ticket.0);
        None
    }));
    assert_eq!(read, (1..=8).collect::<Vec<_>>());
    assert!(!state.poll(now + Duration::from_millis(249), |_| panic!(
        "poll cadence must preserve event-pump budget"
    )));
    read.clear();
    assert!(!state.poll(now + Duration::from_millis(250), |ticket| {
        read.push(ticket.0);
        None
    }));
    assert_eq!(read, [9, 10, 11, 12, 1, 2, 3, 4]);
    assert_eq!(state.pending.len(), 12);
    assert!(state.report()["busy"].as_bool().unwrap());
}

#[test]
fn diagnostics_are_kept_for_display_and_terminal_result_keeps_exact_outcome_and_modules() {
    let mut state = State::default();
    state.start(request("play:one"), |_| Ok(ScriptRecoveryTicket(1)));
    let diagnostic=ara_diag::Diagnostic::error(ara_diag::Code(912),"original rich command fault")
        .data(json!({"module":"Gameplay","play":{"world":"play:one","player":0,"source":"incarnation","binding":1},
            "exception":{"schema":1,"type":"Outer","message":"original","frames":[],"inners":[],"truncated":false}}));
    let mut done = status(1, "play:one", true);
    done.diagnostics.push(diagnostic.clone());
    done.players.push(ScriptRecoveryPlayerResult {
        player: 0,
        source: "incarnation".into(),
        binding: 1,
        outcome: ScriptRecoveryOutcome::Reloaded,
        modules: vec!["Gameplay".into()],
        diagnostics: vec![diagnostic.clone()],
    });
    let mut reply = Some(done);
    assert!(state.poll(Instant::now(), |_| reply.take()));
    let retained = state.terminal.front().unwrap();
    assert_eq!(
        retained.diagnostics.as_slice(),
        std::slice::from_ref(&diagnostic)
    );
    assert_eq!(retained.players[0].diagnostics, [diagnostic]);
    assert_eq!(retained.players[0].modules, ["Gameplay"]);
    assert!(matches!(
        retained.players[0].outcome,
        ScriptRecoveryOutcome::Reloaded
    ));
    // State has no Host/bus handle. Presentation consumes the original diagnostic,
    // and the integration poll must never call report_failure/Cx::log for Status.
    assert!(state::summary(retained).contains("Reloaded"));
}

#[test]
fn duplicate_backend_ticket_and_transport_refusal_do_not_replace_earlier_ownership() {
    let mut state = State::default();
    state.start(request("play:one"), |_| Ok(ScriptRecoveryTicket(1)));
    state.start(request("play:two"), |_| Ok(ScriptRecoveryTicket(1)));
    assert_eq!(state.pending[&1].world, "play:one");
    assert_eq!(state.pending.len(), 1);
    assert!(
        state
            .note(Some("play:two"))
            .unwrap()
            .contains("duplicate owned ticket")
    );
    let diagnostic =
        ara_diag::Diagnostic::error(crate::codes::ACTION, "owned recovery channel closed");
    state.start(request("play:three"), |_| Err(vec![diagnostic.clone()]));
    assert_eq!(state.request_error.as_ref().unwrap().1, [diagnostic]);
    assert_eq!(state.pending.len(), 1);
}

#[test]
fn pending_and_terminal_storage_are_bounded_without_evicting_live_tickets() {
    let mut state = State::default();
    for ticket in 1..=64 {
        assert!(state.start(request(&format!("play:{ticket}")), |_| Ok(
            ScriptRecoveryTicket(ticket)
        )));
    }
    assert!(state.full());
    assert!(!state.start(request("play:overflow"), |_| panic!(
        "do not submit beyond owned ticket capacity"
    )));
    let now = Instant::now();
    for round in 0..8 {
        assert!(
            state.poll(now + Duration::from_millis(round * 250), |ticket| Some(
                status(ticket.0, &format!("play:{}", ticket.0), true)
            ))
        );
    }
    assert!(!state.busy());
    assert_eq!(state.terminal.len(), 8);
    assert_eq!(state.report()["pending"], json!([]));
}

#[test]
fn retained_status_view_escapes_messages_and_bounds_visible_diagnostic_rows() {
    let mut status = status(1, "play:<&>", true);
    for index in 0..40 {
        status.diagnostics.push(ara_diag::Diagnostic::error(
            crate::codes::ACTION,
            format!("failure <&> '{index}'"),
        ));
    }
    let mut markup = String::new();
    view::status_markup(&status, &mut markup);
    assert_eq!(
        markup
            .matches("id='script-recovery-result-1-diagnostic-")
            .count(),
        16
    );
    assert!(markup.contains("24 additional diagnostics"));
    assert!(!markup.contains("failure <&>"));
}
