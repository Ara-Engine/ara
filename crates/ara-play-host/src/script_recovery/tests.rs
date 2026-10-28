use super::*;

fn request() -> ScriptRecoveryRequest {
    ScriptRecoveryRequest {
        session: "editor".into(),
        world: "play:7".into(),
        player: Some(0),
        why: None,
    }
}
fn target() -> Target {
    Target {
        world: "play:7".into(),
        player: 0,
        source: "process-incarnation".into(),
        binding: 1,
    }
}
fn accept(controller: &mut Controller) {
    let Message::Request {
        status,
        targets,
        reply,
    } = controller.rx.try_recv().expect("queued request")
    else {
        panic!("request expected");
    };
    let ticket = status.ticket.0;
    controller.tasks.push(Task {
        status,
        waiting: targets.into_iter().map(|target| (target, ticket)).collect(),
        reply,
        dirty: true,
    });
}

#[test]
fn unknown_world_is_a_located_refusal_and_unknown_ticket_stays_unknown() {
    let mut controller = Controller::default();
    let mut handle = controller.handle(Path::new("project"));
    assert!(handle.poll(ScriptRecoveryTicket(999)).is_none());
    let ticket = handle.request(request()).expect("typed refusal");
    assert!(!handle.poll(ticket).expect("queued").finished);
    let mut published = Vec::new();
    controller.tick(
        &mut BTreeMap::new(),
        &mut crate::scripts::DiagnosticSources::default(),
        Path::new("project"),
        &mut |diagnostic| {
            assert!(
                !handle
                    .poll(ticket)
                    .expect("not finished before ingress")
                    .finished
            );
            published.push(diagnostic);
        },
    );
    let status = handle.poll(ticket).expect("known ticket");
    assert!(status.finished);
    assert!(status.players.is_empty());
    assert!(status.diagnostics[0].location.file.is_some());
    assert_eq!(published.len(), 1);
}

#[test]
fn repeated_requests_coalesce_and_only_matching_incarnation_completion_can_activate() {
    let mut controller = Controller::default();
    let mut handle = controller.handle(Path::new("project"));
    let Message::Subscribe(reply) = controller.rx.try_recv().expect("subscription") else {
        panic!("subscription expected");
    };
    reply
        .try_send(Update::Roster(vec![target()]))
        .expect("roster");
    let ticket = handle.request(request()).expect("request");
    assert_eq!(handle.request(request()).expect("coalesced"), ticket);
    accept(&mut controller);
    let diagnostic = Diagnostic::info(
        ara_script::codes::RECOVERY_RESULT,
        "Script recovery reloaded",
    )
    .data(
        json!({"recovery":{"schema":1,"ticket":ticket.0,"outcome":"reloaded","modules":["Game"]}}),
    );
    controller.observe("play:7", 0, "different-process", &diagnostic);
    assert!(!controller.tasks[0].status.finished);
    let mut sources = crate::scripts::DiagnosticSources::default();
    let scoped = sources
        .scope("play:7", 0, "process-incarnation", diagnostic)
        .expect("trusted scope");
    controller.observe("play:7", 0, "process-incarnation", &scoped);
    controller.tick(
        &mut BTreeMap::new(),
        &mut sources,
        Path::new("project"),
        &mut |_| panic!("scoped script rows must not be published twice"),
    );
    let status = handle.poll(ticket).expect("completion");
    assert!(status.finished);
    assert_eq!(status.players[0].outcome, ScriptRecoveryOutcome::Reloaded);
    assert_eq!(status.players[0].source, "process-incarnation");
    assert_eq!(status.players[0].binding, 1);
    assert_eq!(status.players[0].modules, ["Game"]);
    assert!(
        status.players[0].diagnostics[0]
            .data
            .as_ref()
            .expect("metadata")
            .get("play")
            .is_some()
    );
}

#[test]
fn incarnation_exit_refuses_pending_work_without_fabricating_success() {
    let mut controller = Controller::default();
    let mut handle = controller.handle(Path::new("project"));
    let Message::Subscribe(reply) = controller.rx.try_recv().expect("subscription") else {
        panic!("subscription expected");
    };
    reply
        .try_send(Update::Roster(vec![target()]))
        .expect("roster");
    let ticket = handle.request(request()).expect("request");
    accept(&mut controller);
    controller.tick(
        &mut BTreeMap::new(),
        &mut crate::scripts::DiagnosticSources::default(),
        Path::new("project"),
        &mut |_| {},
    );
    let status = handle.poll(ticket).expect("refusal");
    assert!(status.finished);
    assert_eq!(status.players[0].outcome, ScriptRecoveryOutcome::Refused);
    assert!(status.players[0].diagnostics[0].location.file.is_some());
}

#[test]
fn service_drop_finishes_owned_pending_tickets_as_refused() {
    let controller = Controller::default();
    let mut handle = controller.handle(Path::new("project"));
    let Message::Subscribe(reply) = controller.rx.try_recv().expect("subscription") else {
        panic!("subscription expected");
    };
    reply
        .try_send(Update::Roster(vec![target()]))
        .expect("roster");
    let ticket = handle.request(request()).expect("request");
    drop(controller);
    let status = handle.poll(ticket).expect("retained ticket");
    assert!(status.finished);
    assert_eq!(status.players[0].outcome, ScriptRecoveryOutcome::Refused);
}

#[test]
fn outstanding_requests_are_bounded_and_terminal_history_retires_oldest_tickets() {
    let controller = Controller::default();
    let mut handle = controller.handle(Path::new("project"));
    let Message::Subscribe(reply) = controller.rx.try_recv().expect("subscription") else {
        panic!("subscription expected");
    };
    let roster: Vec<_> = (0..=LIMIT)
        .map(|index| Target {
            world: format!("play:{index}"),
            ..target()
        })
        .collect();
    reply.try_send(Update::Roster(roster)).expect("roster");
    for index in 0..LIMIT {
        handle
            .request(ScriptRecoveryRequest {
                world: format!("play:{index}"),
                ..request()
            })
            .expect("within bound");
    }
    assert!(
        handle
            .request(ScriptRecoveryRequest {
                world: format!("play:{LIMIT}"),
                ..request()
            })
            .is_err()
    );

    let mut controller = Controller::default();
    let mut handle = controller.handle(Path::new("project"));
    let first = handle.request(request()).expect("terminal refusal");
    controller.tick(
        &mut BTreeMap::new(),
        &mut crate::scripts::DiagnosticSources::default(),
        Path::new("project"),
        &mut |_| {},
    );
    handle.poll(first);
    for _ in 0..HISTORY {
        handle.request(request()).expect("bounded history");
        controller.tick(
            &mut BTreeMap::new(),
            &mut crate::scripts::DiagnosticSources::default(),
            Path::new("project"),
            &mut |_| {},
        );
    }
    assert!(
        handle.poll(first).is_none(),
        "retired ticket must never become a successful result"
    );
}
