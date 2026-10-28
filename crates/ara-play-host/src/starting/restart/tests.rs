//! Real owner/player links hold the replacement START while the Host keeps pumping.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_host::{Host, HostConfig, Project};
use ara_play::link::{Link, Message};
use ara_proto::ConnId;
use std::{
    net::TcpListener,
    process::{Command, Stdio},
    thread,
};
mod catalog;
mod queued;

fn pulse(
    link: &mut Link,
    root: &Path,
    reloads: &mut u64,
) -> Result<(), Box<dyn std::error::Error>> {
    link.notify(ara_play::HEARTBEAT, json!({}))?;
    while root.join(format!("reload-{}", *reloads + 1)).is_file() {
        *reloads += 1;
        link.notify(
            ara_play::EVENT,
            ara_play::Event::Reload {
                tick: *reloads,
                level: 1,
                module: 1,
                origin: 0,
                cascade: 1,
                errors: 0,
            },
        )?;
        std::fs::write(root.join(format!("reload-{reloads}-sent")), [])?;
    }
    Ok(())
}

#[test]
fn player_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let Ok(port) = std::env::var("ARA_RESTART_TEST_PORT") else {
        return Ok(());
    };
    let root = PathBuf::from(std::env::var("ARA_RESTART_TEST_ROOT")?);
    let mut link = Link::connect(port.parse()?)?;
    let (mut reloads, mut commands) = (0, 0);
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(20) {
        pulse(&mut link, &root, &mut reloads)?;
        let Some(Message::Request { id, method, params }) = link.wait(Duration::from_millis(10))?
        else {
            continue;
        };
        match method.as_str() {
            method::SNAPSHOT => {
                link.respond(id, Ok(json!({"snapshot":"portable-state-at-tick-42"})))?
            }
            method::START => {
                std::fs::write(
                    root.join("start.json"),
                    ara_json::to_canonical_string(&params),
                )?;
                std::fs::write(root.join("start-entered"), [])?;
                link.notify(
                    ara_play::BUSY,
                    ara_play::Busy {
                        what: "replacement regression gate".into(),
                        max_ms: 120_350,
                    },
                )?;
                let wait = Instant::now();
                while root.join("hold-start").is_file() && !root.join("release").is_file() {
                    if wait.elapsed() > Duration::from_secs(10) {
                        return Err("replacement fixture gate expired".into());
                    }
                    pulse(&mut link, &root, &mut reloads)?;
                    thread::sleep(Duration::from_millis(2));
                }
                link.respond(id, Ok(json!({"tick":42})))?;
            }
            method::COMMANDS => {
                commands += 1;
                std::fs::write(root.join(format!("commands-{commands}-entered")), [])?;
                let wait = Instant::now();
                while root.join("hold-commands").is_file()
                    && !root.join(format!("commands-{commands}-release")).is_file()
                    && !root.join("release").is_file()
                {
                    if wait.elapsed() > Duration::from_secs(10) {
                        return Err("catalog fixture gate expired".into());
                    }
                    pulse(&mut link, &root, &mut reloads)?;
                    thread::sleep(Duration::from_millis(2));
                }
                let catalog = if root.join("invalid-commands").is_file() {
                    Value::Null
                } else {
                    json!({"Fixture.Command":{"summary":"Real-link regression fixture","revision":commands}})
                };
                link.respond(id, Ok(catalog))?;
            }
            method::STOP => {
                link.respond(id, Ok(json!({"tick":42})))?;
                return Ok(());
            }
            _ => link.respond(id, Ok(Value::Null))?,
        }
    }
    Err("restart fixture exceeded its lifetime".into())
}

struct Fixture {
    root: tempfile::TempDir,
    _process_lease: std::sync::MutexGuard<'static, ()>,
}
impl Fixture {
    fn new() -> Self {
        // Keep owned child lifetimes together: another test's SIGCHLD can
        // interrupt a blocking WebSocket handshake on Linux.
        static PROCESSES: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let process_lease = PROCESSES.lock().unwrap_or_else(|error| error.into_inner());
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .join("ara-play-host/restart");
        std::fs::create_dir_all(&target).unwrap();
        let root = tempfile::Builder::new()
            .prefix("restart-")
            .tempdir_in(target)
            .unwrap();
        std::fs::create_dir_all(root.path().join(ara_doc::AUTHORING_ROOT)).unwrap();
        std::fs::write(
            root.path().join("ara.project.json"),
            ara_json::to_canonical_string(&json!({
                "layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Restart regression"
            })),
        )
        .unwrap();
        Self {
            root,
            _process_lease: process_lease,
        }
    }
    fn player(&self, name: &str, held: bool) -> Player {
        let root = self.root.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        if held {
            std::fs::write(root.join("hold-start"), []).unwrap()
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "starting::restart::tests::player_fixture",
                "--nocapture",
            ])
            .env(
                "ARA_RESTART_TEST_PORT",
                listener.local_addr().unwrap().port().to_string(),
            )
            .env("ARA_RESTART_TEST_ROOT", &root)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW for the owned test child.
        }
        let mut child = command.spawn().unwrap();
        let pid = child.id();
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("owned replacement fixture failed to connect")
                    }
                    thread::yield_now();
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("fixture accept: {error}")
                }
            }
        };
        Player {
            incarnation: format!("restart-test-{name}-{pid}"),
            child: child.into(),
            link: ara_play::link::Pump::owner(Link::accept(stream).unwrap()).expect("owner IO"),
            hello: ara_play::Hello {
                token: "restart-test".into(),
                protocol: ara_play::PROTOCOL.into(),
                pid,
                name: name.into(),
                window: false,
                init_ms: 0.,
            },
            log: root.join("player.log"),
            seen: Instant::now(),
            status: None,
            grace: Instant::now() + Duration::from_secs(10),
            events: Vec::new(),
            embed: Vec::new(),
            replies: Default::default(),
            profiling: Vec::new(),
        }
    }
    fn service(&self, held: bool, internal: bool) -> PlayService {
        let mut old = self.player("old", false);
        if internal {
            old.events.push(ara_play::Event::Reload {
                tick: 42,
                level: 4,
                module: 1,
                origin: 0,
                cascade: 1,
                errors: 0,
            });
            old.events.push(ara_play::Event::Restart);
        }
        let replacement = self.player("replacement", held);
        let mut config = PlayConfig::for_project(self.root.path());
        config.pool.headless = 0;
        config.pool.windowed = 0;
        config.scripts = None;
        config.import_worker = self.root.path().join("missing-import-worker");
        let mut service = PlayService::new(self.root.path(), config.clone());
        service.pool =
            Some(Pool::with_idle(config.pool, "restart-test".into(), vec![replacement]).unwrap());
        service.assets =
            Some(assets::Assets::new(self.root.path(), &config.import_worker).unwrap());
        service.next = 1;
        service.worlds.insert("play:1".into(), world(old));
        service
    }
    fn host(&self, service: PlayService) -> (Host, Value) {
        let mut host = Host::open(
            Project::open(self.root.path()).unwrap(),
            HostConfig::default(),
        )
        .unwrap();
        host.register(Box::new(service)).unwrap();
        let session = host
            .call(
                ConnId::LOCAL,
                "session.begin",
                json!({"author":"human","client":"test"}),
            )
            .unwrap()["session"]
            .clone();
        (host, session)
    }
    fn release(&self) {
        let _ = std::fs::write(self.root.path().join("replacement/release"), []);
        let _ = std::fs::write(self.root.path().join("old/release"), []);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.release();
    }
}

fn world(player: Player) -> World {
    World {
        players: vec![(Role::Local, player)],
        scene: "fixture".into(),
        snapshot: Vec::new(),
        section: None,
        window: false,
        process_mode: ProcessMode::Separate,
        embed: None,
        surfaces: vec![None],
        start_paused: false,
        fault: None,
        hot_reload: false,
        streaming_assets: false,
        import_revision: 0,
        commands: Value::Null,
        startup: Value::Null,
        events: Vec::new(),
        conditions: Vec::new(),
    }
}

fn status(host: &mut Host, session: &Value) -> Value {
    host.call(ConnId::LOCAL, "play.status", json!({"session":session}))
        .unwrap()
}
fn pump_until(host: &mut Host, session: &Value, mut ready: impl FnMut(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let began = Instant::now();
        host.tick();
        let state = status(host, session);
        assert!(
            began.elapsed() < Duration::from_millis(250),
            "owner callback blocked while replacement waited"
        );
        if ready(&state) {
            return state;
        }
        assert!(Instant::now() < deadline, "{state}");
        thread::sleep(Duration::from_millis(2));
    }
}
fn stop_and_drain(host: &mut Host, session: &Value) {
    host.call(
        ConnId::LOCAL,
        "play.stop",
        json!({"session":session,"world":"play:1"}),
    )
    .unwrap();
    pump_until(host, session, |state| {
        state["worlds"].as_array().unwrap().is_empty()
            && state["stopping"].as_array().unwrap().is_empty()
    });
}

#[test]
fn background_restart_keeps_the_owner_responsive_and_coalesces_a_held_replacement() {
    let fixture = Fixture::new();
    let service = fixture.service(true, false);
    let (mut host, session) = fixture.host(service);
    let authoring_revision = host.store().revision();
    let began = Instant::now();
    let ack = host
        .call(
            ConnId::LOCAL,
            "play.restart",
            json!({"session":session,"world":"play:1","background":true,"keepState":true}),
        )
        .unwrap();
    assert!(began.elapsed() < Duration::from_millis(250));
    assert_eq!(ack["status"], "starting");
    assert_eq!(ack["restart"], true);
    pump_until(&mut host, &session, |_| {
        fixture
            .root
            .path()
            .join("replacement/start-entered")
            .is_file()
    });
    for _ in 0..20 {
        let ack = host
            .call(
                ConnId::LOCAL,
                "play.restart",
                json!({"session":session,"world":"play:1","background":true}),
            )
            .unwrap();
        assert_eq!(ack["coalesced"], true);
        assert_eq!(ack["restart"], true);
        let began = Instant::now();
        host.tick();
        let state = status(&mut host, &session);
        assert!(began.elapsed() < Duration::from_millis(250));
        assert_eq!(state["worlds"].as_array().unwrap().len(), 1);
        assert_eq!(state["worlds"][0]["restart"], true);
        assert_eq!(state["worlds"][0]["status"], "starting");
    }
    let start: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.root.path().join("replacement/start.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(start["resume"], "portable-state-at-tick-42");
    assert!(
        !fixture.root.path().join("old/commands-1-entered").is_file(),
        "L4 must not queue old-generation COMMANDS ahead of its snapshot"
    );
    fixture.release();
    let state = pump_until(&mut host, &session, |state| {
        state["worlds"][0]["status"] == "running"
    });
    assert_eq!(state["worlds"][0]["players"].as_array().unwrap().len(), 1);
    assert_eq!(host.store().revision(), authoring_revision);
    stop_and_drain(&mut host, &session);
}

#[test]
fn cancelling_a_held_restart_returns_before_the_player_reply_and_reaps_owned_work() {
    let fixture = Fixture::new();
    let service = fixture.service(true, false);
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
    // Leave the real replacement gate closed: cancellable call_checked must break
    // out and destroy/reap its owned child without requiring a START response.
    pump_until(&mut host, &session, |state| {
        state["worlds"].as_array().unwrap().is_empty()
            && state["stopping"].as_array().unwrap().is_empty()
    });
    assert!(!fixture.root.path().join("replacement/release").is_file());
}

#[test]
fn an_internal_l4_event_defers_snapshot_and_replacement_without_a_background_request() {
    let fixture = Fixture::new();
    let service = fixture.service(true, true);
    let (mut host, session) = fixture.host(service);
    let began = Instant::now();
    host.tick();
    assert!(
        began.elapsed() < Duration::from_millis(250),
        "L4 tick ran synchronous resume/restart"
    );
    let state = status(&mut host, &session);
    assert_eq!(state["worlds"][0]["restart"], true);
    pump_until(&mut host, &session, |_| {
        fixture
            .root
            .path()
            .join("replacement/start-entered")
            .is_file()
    });
    let start: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.root.path().join("replacement/start.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(start["resume"], "portable-state-at-tick-42");
    assert!(
        !fixture.root.path().join("old/commands-1-entered").is_file(),
        "L4 defers catalog refresh until the new player is ready"
    );
    fixture.release();
    pump_until(&mut host, &session, |state| {
        state["worlds"][0]["status"] == "running"
    });
    stop_and_drain(&mut host, &session);
}

#[test]
fn omitted_background_preserves_the_synchronous_ready_world_response() {
    let fixture = Fixture::new();
    let service = fixture.service(false, false);
    let (mut host, session) = fixture.host(service);
    let result = host
        .call(
            ConnId::LOCAL,
            "play.restart",
            json!({"session":session,"world":"play:1"}),
        )
        .unwrap();
    assert_eq!(result["world"], "play:1");
    assert!(result["bootMs"].is_number());
    assert_eq!(result["players"].as_array().unwrap().len(), 1);
    assert_eq!(result["status"], Value::Null);
    assert_eq!(
        status(&mut host, &session)["worlds"][0]["status"],
        "running"
    );
    stop_and_drain(&mut host, &session);
}
