#![allow(clippy::unwrap_used)]
use ara_bridge::{
    BridgeError, Command, DeviceClient, InputAction, LogRecord, RuntimeHooks, Server, Telemetry,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, net::SocketAddr, sync::mpsc};
const TOKEN: &str = "private-test-token-32-bytes-minimum";
struct Hooks(mpsc::Sender<InputAction>);
impl RuntimeHooks for Hooks {
    fn screenshot(&mut self) -> Result<Vec<u8>, BridgeError> {
        Ok(b"\x89PNG\r\n\x1a\nfixture".to_vec())
    }
    fn input(&mut self, a: InputAction) -> Result<(), BridgeError> {
        self.0
            .send(a)
            .map_err(|_| BridgeError::new("ARA-EXP-BRG-011", "sim disconnected", "restart runtime"))
    }
    fn scene(&mut self, q: Value) -> Result<Value, BridgeError> {
        Ok(json!({"entities":[{"id":"e_test"}],"query":q}))
    }
    fn capture(&mut self, kind: &str) -> Result<Value, BridgeError> {
        Ok(json!({"queued":kind}))
    }
}
#[test]
fn websocket_logs_perf_screenshot_scene_capture_and_tick_input() {
    let (tx, rx) = mpsc::channel();
    let server = Server::start(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        TOKEN.into(),
        Hooks(tx),
    )
    .unwrap();
    assert!(server.publisher().try_publish(Telemetry {
        logs: vec![LogRecord {
            sequence: 99,
            tick: 10,
            level: "info".into(),
            target: "test".into(),
            message: "frame".into(),
            fields: BTreeMap::new()
        }],
        frame_us: vec![1000., 2000.],
        gpu_us: vec![500.]
    }));
    let mut client = DeviceClient::connect(server.address(), TOKEN.into()).unwrap();
    assert_eq!(client.request(Command::Hello).unwrap()["protocol"], 1);
    assert_eq!(client.logs(0).unwrap()["logs"][0]["sequence"], 1);
    assert_eq!(client.perf().unwrap().frame.samples, 2);
    assert!(client.screenshot().unwrap().starts_with(b"\x89PNG"));
    assert_eq!(
        client
            .request(Command::Scene {
                query: json!({"tag":"player"})
            })
            .unwrap()["entities"][0]["id"],
        "e_test"
    );
    assert_eq!(
        client
            .request(Command::Capture { kind: "gpu".into() })
            .unwrap()["queued"],
        "gpu"
    );
    let action = InputAction {
        action: "jump".into(),
        values: vec![1.],
        tick: 42,
    };
    client.request(Command::Input(action.clone())).unwrap();
    assert_eq!(rx.recv().unwrap(), action);
    assert!(
        client
            .request(Command::Input(InputAction {
                action: "jump".into(),
                values: vec![2.],
                tick: 42
            }))
            .is_err()
    );
}
#[test]
fn unauthenticated_and_unintegrated_operations_fail_with_stable_codes() {
    let server = Server::start(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        TOKEN.into(),
        ara_bridge::NoHooks,
    )
    .unwrap();
    {
        let mut c = DeviceClient::connect(server.address(), "wrong".into()).unwrap();
        assert!(
            c.perf()
                .unwrap_err()
                .to_string()
                .contains("ARA-EXP-BRG-006")
        );
    }
    let mut c = DeviceClient::connect(server.address(), TOKEN.into()).unwrap();
    assert!(
        c.screenshot()
            .unwrap_err()
            .to_string()
            .contains("ARA-EXP-BRG-010")
    );
    assert!(
        c.request(Command::Scene { query: Value::Null })
            .unwrap_err()
            .to_string()
            .contains("ARA-EXP-BRG-012")
    );
}
#[test]
fn telemetry_backpressure_and_histograms_are_bounded() {
    let server = Server::start(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        TOKEN.into(),
        ara_bridge::NoHooks,
    )
    .unwrap();
    let publisher = server.publisher();
    for _ in 0..500 {
        publisher.try_publish(Telemetry::default());
    }
    let mut h = ara_bridge::Histogram::default();
    for us in [f64::NAN, -1., 100., 1000000.] {
        h.record(us);
    }
    assert_eq!(h.samples, 2);
    assert_eq!(h.counts[0], 1);
    assert_eq!(h.counts[10], 1);
    assert!(
        Server::start(
            SocketAddr::from(([0, 0, 0, 0], 0)),
            TOKEN.into(),
            ara_bridge::NoHooks
        )
        .is_err()
    );
    assert!(
        Server::start(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            "short".into(),
            ara_bridge::NoHooks
        )
        .is_err()
    );
}

#[test]
fn idle_connections_do_not_block_other_editor_or_agent_clients() {
    let server = Server::start(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        TOKEN.into(),
        ara_bridge::NoHooks,
    )
    .unwrap();
    let mut idle = DeviceClient::connect(server.address(), TOKEN.into()).unwrap();
    idle.request(Command::Hello).unwrap();
    let mut active = DeviceClient::connect(server.address(), TOKEN.into()).unwrap();
    assert_eq!(active.request(Command::Hello).unwrap()["protocol"], 1);
    assert!(active.perf().is_ok());
    assert!(idle.perf().is_ok());
}
