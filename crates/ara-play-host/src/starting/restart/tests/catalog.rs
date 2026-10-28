//! Real hung catalog RPCs coalesce reloads and never publish obsolete replies.
use super::*;

fn revision(host: &mut Host, session: &Value) -> u64 {
    host.call(
        ConnId::LOCAL,
        "schema.describe",
        json!({"session":session,"name":"Fixture.Command"}),
    )
    .unwrap()["items"][0]["revision"]
        .as_u64()
        .unwrap()
}
fn signal(fixture: &Fixture, name: &str) {
    std::fs::write(fixture.root.path().join("old").join(name), []).unwrap()
}
fn initial_catalog(fixture: &Fixture, host: &mut Host, session: &Value) {
    signal(fixture, "reload-1");
    pump_until(host, session, |state| {
        state["worlds"][0]["commandRefresh"]["publishedGeneration"]
            .as_u64()
            .is_some_and(|n| n > 0)
    });
    assert_eq!(revision(host, session), 1);
}

#[test]
fn a_hung_catalog_keeps_last_good_visible_and_only_newest_coalesced_generation_publishes() {
    let fixture = Fixture::new();
    let service = fixture.service(false, false);
    let (mut host, session) = fixture.host(service);
    initial_catalog(&fixture, &mut host, &session);
    signal(&fixture, "hold-commands");
    signal(&fixture, "reload-2");
    let state = pump_until(&mut host, &session, |_| {
        fixture.root.path().join("old/commands-2-entered").is_file()
    });
    assert_eq!(state["worlds"][0]["commandsRefreshing"], true);
    assert_eq!(state["worlds"][0]["commandsStale"], true);
    let published = state["worlds"][0]["commandRefresh"]["publishedGeneration"].clone();
    let in_flight = state["worlds"][0]["commandRefresh"]["generation"]
        .as_u64()
        .unwrap();
    signal(&fixture, "reload-3");
    signal(&fixture, "reload-4");
    pump_until(&mut host, &session, |state| {
        state["worlds"][0]["commandRefresh"]["generation"]
            .as_u64()
            .is_some_and(|n| n > in_flight)
    });
    assert_eq!(
        revision(&mut host, &session),
        1,
        "in-flight refresh must preserve the last-good registry"
    );
    signal(&fixture, "commands-2-release");
    let state = pump_until(&mut host, &session, |_| {
        fixture.root.path().join("old/commands-3-entered").is_file()
    });
    assert_eq!(
        state["worlds"][0]["commandRefresh"]["publishedGeneration"],
        published
    );
    assert_eq!(
        revision(&mut host, &session),
        1,
        "the obsolete second reply was never published"
    );
    assert!(
        !fixture.root.path().join("old/commands-4-entered").is_file(),
        "Reloads coalesce into one fresh RPC"
    );
    signal(&fixture, "commands-3-release");
    let state = pump_until(&mut host, &session, |state| {
        state["worlds"][0]["commandsRefreshing"] == false
            && state["worlds"][0]["commandsStale"] == false
    });
    assert_eq!(
        state["worlds"][0]["commandRefresh"]["publishedGeneration"],
        state["worlds"][0]["commandRefresh"]["generation"]
    );
    assert_eq!(revision(&mut host, &session), 3);
    stop_and_drain(&mut host, &session);
}

#[test]
fn stopping_discards_a_pending_old_authority_refresh_without_waiting_for_its_reply() {
    let fixture = Fixture::new();
    let service = fixture.service(false, false);
    let (mut host, session) = fixture.host(service);
    initial_catalog(&fixture, &mut host, &session);
    signal(&fixture, "hold-commands");
    signal(&fixture, "reload-2");
    pump_until(&mut host, &session, |_| {
        fixture.root.path().join("old/commands-2-entered").is_file()
    });
    let began = Instant::now();
    let result = host
        .call(
            ConnId::LOCAL,
            "play.stop",
            json!({"session":session,"world":"play:1"}),
        )
        .unwrap();
    assert!(began.elapsed() < Duration::from_millis(250));
    assert_eq!(result["status"], "stopping");
    signal(&fixture, "commands-2-release");
    pump_until(&mut host, &session, |state| {
        state["worlds"].as_array().unwrap().is_empty()
            && state["stopping"].as_array().unwrap().is_empty()
    });
    assert!(
        host.call(
            ConnId::LOCAL,
            "schema.describe",
            json!({"session":session,"name":"Fixture.Command"})
        )
        .is_err(),
        "late reply cannot republish a stopped world's commands"
    );
}

#[test]
fn malformed_refresh_keeps_last_good_and_exposes_a_real_stale_error() {
    let fixture = Fixture::new();
    let service = fixture.service(false, false);
    let (mut host, session) = fixture.host(service);
    initial_catalog(&fixture, &mut host, &session);
    signal(&fixture, "invalid-commands");
    signal(&fixture, "reload-2");
    let state = pump_until(&mut host, &session, |state| {
        state["worlds"][0]["commandRefresh"]["error"].is_string()
    });
    assert_eq!(state["worlds"][0]["commandsRefreshing"], false);
    assert_eq!(state["worlds"][0]["commandsStale"], true);
    assert!(
        state["worlds"][0]["commandRefresh"]["error"]
            .as_str()
            .unwrap()
            .contains("non-object")
    );
    assert_eq!(revision(&mut host, &session), 1);
    stop_and_drain(&mut host, &session);
}
