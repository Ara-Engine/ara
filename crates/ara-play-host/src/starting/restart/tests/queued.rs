//! A queued restart keeps its old child owned and can retire it independently.
use super::*;

#[test]
fn queued_restart_coalesces_and_stop_retires_it_while_another_replacement_is_held() {
    let fixture = Fixture::new();
    let mut service = fixture.service(true, false);
    service
        .worlds
        .insert("play:2".into(), world(fixture.player("second", false)));
    service.next = 2;
    let (mut host, session) = fixture.host(service);
    host.call(
        ConnId::LOCAL,
        "play.restart",
        json!({"session":session,"world":"play:1","background":true}),
    )
    .unwrap();
    pump_until(&mut host, &session, |_| {
        fixture
            .root
            .path()
            .join("replacement/start-entered")
            .is_file()
    });
    let result = host
        .call(
            ConnId::LOCAL,
            "play.restart",
            json!({"session":session,"world":"play:2","background":true}),
        )
        .unwrap();
    assert_eq!(result["restart"], true);
    let result = host
        .call(
            ConnId::LOCAL,
            "play.restart",
            json!({"session":session,"world":"play:2","background":true}),
        )
        .unwrap();
    assert_eq!(result["coalesced"], true);
    let state = status(&mut host, &session);
    let second = state["worlds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|world| world["world"] == "play:2")
        .unwrap();
    assert_eq!(second["restart"], true);
    assert_eq!(second["status"], "starting");
    let began = Instant::now();
    assert_eq!(
        host.call(
            ConnId::LOCAL,
            "play.stop",
            json!({"session":session,"world":"play:2"})
        )
        .unwrap()["status"],
        "stopping"
    );
    assert!(began.elapsed() < Duration::from_millis(250));
    let state = pump_until(&mut host, &session, |state| {
        state["worlds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|world| world["world"] != "play:2")
            && state["stopping"].as_array().unwrap().is_empty()
    });
    assert_eq!(
        state["worlds"][0]["status"], "starting",
        "other restart remains held"
    );
    assert!(!fixture.root.path().join("second/start-entered").is_file());
    fixture.release();
    pump_until(&mut host, &session, |state| {
        state["worlds"][0]["status"] == "running"
    });
    stop_and_drain(&mut host, &session);
}
