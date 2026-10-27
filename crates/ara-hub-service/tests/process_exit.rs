//! Windowless editor authorities exercise real OS process exits, including stale discovery.
use ara_hub_service::{DirectoryTemplates, EngineBuild, Hub, HubConfig};
use ara_proto::{ConnId, Endpoint, Incoming, jsonrpc};
use ara_rpc::{Client, DaemonInfo, Handler, ServeOptions};
use serde_json::{Value, json};
use std::{
    fs,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Editor {
    stop: bool,
    crash: bool,
}
impl Handler for Editor {
    fn message(&mut self, _: ConnId, text: &str) -> Option<String> {
        let Ok(Incoming::Request { id, method, .. }) = Incoming::parse(text) else {
            return None;
        };
        let value = match method.as_str() {
            "rpc.discover" => json!({"methods":[{"name":"editor.state"}]}),
            "session.begin" => json!({"session":"fixture"}),
            "collab.activity" => json!({"participants":[],"presence":{}}),
            "daemon.shutdown" | "fixture.window.close" | "fixture.crash" => {
                self.stop = true;
                self.crash = method == "fixture.crash";
                json!({"stopping":true})
            }
            _ => json!({}),
        };
        Some(jsonrpc::reply(id, Ok(value)).to_string())
    }
    fn stopping(&self) -> bool {
        self.stop
    }
}

#[test]
fn fixture_editor() {
    let Some(root) = std::env::var_os("ARA_HUB_FIXTURE_EDITOR") else {
        return;
    };
    let root = PathBuf::from(root);
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .canonicalize()
        .expect("target");
    assert!(
        root.canonicalize()
            .expect("fixture root")
            .starts_with(target),
        "fixture editor must stay under target/"
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("fixture listener");
    let info = DaemonInfo {
        pid: std::process::id(),
        port: listener.local_addr().expect("port").port(),
        token: "fixture-token".into(),
        protocol: ara_proto::PROTOCOL.into(),
    };
    info.write(&root).expect("discovery");
    let mut editor = Editor {
        stop: false,
        crash: false,
    };
    ara_rpc::serve(
        &mut editor,
        ServeOptions {
            listener: Some(listener),
            token: info.token,
            stdio: false,
            tick: Duration::from_millis(20),
        },
    )
    .expect("fixture serve");
    if editor.crash {
        std::process::exit(7);
    }
    DaemonInfo::remove(&root);
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn editor_exit_notifies_without_discovery_polling() {
    for way in ["hub-close", "window-x", "crash", "kill"] {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/hub-process-tests")
            .join(format!("{}-{way}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("root");
        let root = root.canonicalize().expect("root");
        let exe = std::env::current_exe().expect("exe");
        let mut hub = Hub::open(HubConfig {
            directory: root.join("config"),
            engine: EngineBuild {
                version: "fixture".into(),
                executable: exe.clone(),
                editor: Some(exe.clone()),
            },
            templates: Box::new(DirectoryTemplates(root.join("templates"))),
        })
        .expect("hub");
        let project = root.join("project");
        fs::create_dir(&project).expect("project");
        fs::write(
            project.join("ara.project.json"),
            ara_json::to_canonical_string(&json!({"name":"Fixture","engineVersion":"fixture"})),
        )
        .expect("marker");
        let registered = hub
            .call("hub.register", json!({"path":project}))
            .expect("register");
        let mut command = Command::new(exe);
        command
            .args(["--exact", "fixture_editor", "--nocapture"])
            .env("ARA_HUB_FIXTURE_EDITOR", &project)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut process = Process(command.spawn().expect("fixture process"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut editor = loop {
            if let Some(client) = Client::for_project(&project) {
                break client;
            }
            assert!(Instant::now() < deadline, "fixture ready: {way}");
            std::thread::yield_now();
        };
        let id = registered["id"].clone();
        assert_eq!(
            hub.call("hub.status", json!({"project":id}))
                .expect("status")["headless"],
            false
        );
        hub.call_for(ConnId(9), "hub.subscribe", json!({}))
            .expect("subscribe");
        hub.outbox();
        assert_eq!(
            hub.call("hub.shutdown", json!({"onlyIfIdle":true}))
                .expect("busy editor is retained")["stopping"],
            false
        );
        match way {
            "hub-close" => {
                hub.call("hub.close", json!({"project":id}))
                    .expect("Hub close");
            }
            "window-x" => {
                editor.call("fixture.window.close", json!({})).expect("X");
            }
            "crash" => {
                editor.call("fixture.crash", json!({})).expect("crash");
            }
            "kill" => {
                process.0.kill().expect("kill");
            }
            _ => unreachable!(),
        }
        process.0.wait().expect("exit");
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut closed = false;
        while Instant::now() < deadline && !closed {
            ara_rpc::Handler::tick(&mut hub);
            closed = hub.outbox().into_iter().any(|(_, text)| {
                let note: Value = serde_json::from_str(&text).expect("notification");
                note["method"] == "hub.changed"
                    && note["params"]["kind"] == "closed"
                    && note["params"]["project"] == id
            });
            std::thread::yield_now();
        }
        assert!(
            closed,
            "{way}: process exit must emit project-specific closed notification"
        );
        assert_eq!(
            hub.call("hub.status", json!({"project":id}))
                .expect("status")["state"],
            "closed",
            "{way}"
        );
        if matches!(way, "crash" | "kill") {
            assert!(
                DaemonInfo::read(&project).is_some(),
                "fixture keeps stale discovery"
            );
        }
    }
}
