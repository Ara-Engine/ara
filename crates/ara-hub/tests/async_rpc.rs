//! A held service reply cannot hold the UI thread or prevent input activation.
use ara_hub::{
    capture,
    connection::{Connection, Event},
    model::{Call, Hub, Intent, Prefs},
    native::App,
};
use ara_proto::{ConnId, Incoming, jsonrpc};
use ara_rpc::{DaemonInfo, Handler, ServeOptions};
use ara_ui::input::InputEvent;
use serde_json::json;
use std::{
    net::TcpListener,
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[test]
fn bootstrap_preparation_cannot_hold_the_first_frame_or_connecting_input() {
    let (entered, waiting) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let mut connection = Connection::start_preparing_with_wake(
        "target/hub-async-tests/preparation".into(),
        move || {
            entered.send(()).expect("UI waiting for preparation");
            held.recv_timeout(Duration::from_secs(10))
                .expect("UI releases preparation");
            Err(ara_diag::Diagnostic::error(
                ara_diag::codes::DAEMON_UNREACHABLE,
                "held test preparation finished",
            )
            .into())
        },
        || {},
    );
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("preparation started");
    assert!(connection.poll().is_empty());
    let mut hub = Hub::new(Prefs::default(), "target/hub-async-tests".into(), None);
    let mut app = App::new([1440., 900.], Vec::new()).expect("UI");
    app.rebuild(&mut hub, capture::NOW).expect("first view");
    assert!(app.frame().expect("first frame"));
    for id in ["theme", "nav-settings"] {
        let rect = app.ui.nodes[app.ui.find(id).expect("control")].rect;
        let at = [rect.x + rect.w / 2., rect.y + rect.h / 2.];
        app.dispatch_pointer(&mut hub, InputEvent::PointerDown(at), capture::NOW)
            .expect("down");
        app.dispatch_pointer(&mut hub, InputEvent::PointerUp(at), capture::NOW)
            .expect("up");
    }
    assert!(hub.prefs.light);
    assert_eq!(hub.view, ara_hub::model::View::Settings);
    assert_eq!(hub.link, ara_hub::model::Link::Connecting);
    // Neither the first frame nor either input activation depends on this release.
    release.send(()).expect("release preparation");
    connection.stop();
}

struct SlowService {
    build: String,
    announced: bool,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
impl Handler for SlowService {
    fn message(&mut self, _: ConnId, text: &str) -> Option<String> {
        let Ok(Incoming::Request { id, method, .. }) = Incoming::parse(text) else {
            return None;
        };
        if method == "hub.status" {
            if !self.announced {
                self.announced = true;
                return Some(
                    jsonrpc::reply(id, Ok(json!({"build":self.build,"projects":[]}))).to_string(),
                );
            }
            self.entered.send(()).expect("test waiting");
            self.release
                .recv_timeout(Duration::from_secs(10))
                .expect("UI releases reply");
        }
        Some(jsonrpc::reply(id, Ok(json!({"revision":0,"projects":[]}))).to_string())
    }
}

#[test]
fn a_slow_rpc_does_not_block_ui_input() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/hub-async-tests")
        .join(format!("{}-{stamp}", std::process::id()));
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
    DaemonInfo {
        pid: std::process::id(),
        port: listener.local_addr().expect("port").port(),
        token: "test-token".into(),
        protocol: ara_proto::PROTOCOL.into(),
    }
    .write(&directory.join("hub"))
    .expect("discovery");
    let (entered, waiting) = mpsc::channel();
    let (release, held) = mpsc::channel();
    // The atomic flag provides a clean shutdown for this in-process test server.
    let server = SlowService {
        build: ara_update::sha256_file(&std::env::current_exe().expect("exe")).expect("build"),
        announced: false,
        entered,
        release: held,
    };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    struct Server {
        service: SlowService,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl Handler for Server {
        fn message(&mut self, conn: ConnId, text: &str) -> Option<String> {
            self.service.message(conn, text)
        }
        fn stopping(&self) -> bool {
            self.stop.load(std::sync::atomic::Ordering::Acquire)
        }
    }
    let flag = stop.clone();
    let worker = std::thread::spawn(move || {
        ara_rpc::serve(
            &mut Server {
                service: server,
                stop: flag,
            },
            ServeOptions {
                listener: Some(listener),
                token: "test-token".into(),
                stdio: false,
                tick: Duration::from_millis(10),
            },
        )
        .expect("serve")
    });
    let mut connection =
        Connection::start(directory.clone(), std::env::current_exe().expect("exe"));
    assert!(matches!(
        connection.wait(Duration::from_secs(10)),
        Some(Event::Connected)
    ));
    let mut hub = Hub::new(Prefs::default(), directory.clone(), None);
    capture::seed(&mut hub);
    let mut app = App::new([1440., 900.], Vec::new()).expect("app");
    app.rebuild(&mut hub, capture::NOW).expect("view");
    connection.send([Call::Status]);
    waiting
        .recv_timeout(Duration::from_secs(10))
        .expect("RPC in flight");
    let rect = app.ui.nodes[app.ui.find("theme").expect("theme")].rect;
    let at = [rect.x + rect.w / 2., rect.y + rect.h / 2.];
    app.pointer(InputEvent::PointerDown(at)).expect("down");
    app.pointer(InputEvent::PointerUp(at)).expect("up");
    assert_eq!(
        app.actions(),
        [Intent::ToggleTheme],
        "input completes before the RPC is released"
    );
    release.send(()).expect("release");
    assert!(matches!(
        connection.wait(Duration::from_secs(10)),
        Some(Event::Reply { .. })
    ));
    connection.stop();
    stop.store(true, std::sync::atomic::Ordering::Release);
    worker.join().expect("server stop");
    DaemonInfo::remove(&directory.join("hub"));
}
