//! Player shutdown work stays outside the editor's service-call frame.
use crate::*;
use std::{sync::mpsc, thread};

pub(super) struct Retiring {
    pub id: String,
    pub began: Instant,
    worker: thread::JoinHandle<Value>,
}

impl Retiring {
    /// Start the worker before transferring the world, so spawn failure preserves
    /// the running world and all its surface/player ownership on the service.
    pub fn new(id: &str) -> Result<(Self, mpsc::Sender<World>), Vec<Diagnostic>> {
        let (sender, receiver) = mpsc::channel::<World>();
        let worker = thread::Builder::new()
            .name("ara-play-stop".into())
            .spawn(move || match receiver.recv() {
                Ok(world) => shutdown(world),
                Err(_) => Value::Null,
            })
            .map_err(|error| {
                fail(
                    codes::PLAY,
                    format!("cannot start shutdown worker: {error}"),
                )
            })?;
        Ok((
            Self {
                id: id.to_owned(),
                began: Instant::now(),
                worker,
            },
            sender,
        ))
    }
}

fn shutdown(mut world: World) -> Value {
    ara_gpu::cpu_scope!("Play.shutdown_worker");
    let mut tick = Value::Null;
    for (_, player) in &mut world.players {
        if let Ok(Ok(result)) = player.call(method::STOP, &json!({}), Duration::from_secs(2)) {
            tick = result["tick"].clone();
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(500)
            && !matches!(player.child.try_wait(), Ok(Some(_)))
        {
            thread::sleep(Duration::from_millis(2));
        }
    }
    // Player::drop kills/reaps any child still alive. This worker owns every
    // player, snapshot and surface handle until cleanup has completed.
    drop(world);
    tick
}

impl PlayService {
    /// Poll finished shutdowns only; joining a still-running child is never UI work.
    pub(super) fn collect_stopped(&mut self) -> Vec<Diagnostic> {
        let mut problems = Vec::new();
        let mut index = 0;
        while index < self.retiring.len() {
            if !self.retiring[index].worker.is_finished() {
                index += 1;
                continue;
            }
            let task = self.retiring.swap_remove(index);
            if task.worker.join().is_err() {
                problems.push(Diagnostic::warning(
                    codes::PLAY,
                    format!("{}: shutdown worker failed", task.id),
                ));
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use ara_play::link::{Link, Message};
    use std::{net::TcpListener, process::Command, thread};

    fn wait_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.is_file() {
            if Instant::now() >= deadline {
                return Err(format!("fixture gate timed out: {}", path.display()).into());
            }
            thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }

    #[test]
    fn stop_worker() -> Result<(), Box<dyn std::error::Error>> {
        let Ok(port) = std::env::var("ARA_LIFECYCLE_TEST_PORT") else {
            return Ok(());
        };
        let root = PathBuf::from(std::env::var("ARA_LIFECYCLE_TEST_ROOT")?);
        let mut link = Link::connect(port.parse()?)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(Message::Request { id, method, .. }) =
                link.wait(Duration::from_millis(20))?
                && method == method::STOP
            {
                link.notify(
                    ara_play::BUSY,
                    ara_play::Busy {
                        what: "shutdown fixture".into(),
                        max_ms: 5_000,
                    },
                )?;
                std::fs::write(root.join("stop-entered"), [])?;
                wait_file(&root.join("release"))?;
                link.respond(id, Ok(json!({"tick":42})))?;
                return Ok(());
            }
        }
        Err("fixture never received stop".into())
    }

    struct Release(PathBuf);
    impl Drop for Release {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, []);
        }
    }

    #[test]
    fn stop_does_not_wait_for_the_player_shutdown_reply() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-play-host/lifecycle")
            .join(format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root)?;
        let release = Release(root.join("release"));
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--exact", "lifecycle::tests::stop_worker", "--nocapture"])
            .env(
                "ARA_LIFECYCLE_TEST_PORT",
                listener.local_addr()?.port().to_string(),
            )
            .env("ARA_LIFECYCLE_TEST_ROOT", &root)
            .stdout(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW, including the fixture.
        }
        let child = command.spawn()?;
        let pid = child.id();
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "fixture did not connect");
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(error.into()),
            }
        };
        let player = Player {
            child,
            link: Link::accept(stream)?,
            hello: ara_play::Hello {
                token: "test".into(),
                protocol: ara_play::PROTOCOL.into(),
                pid,
                name: "stop-test".into(),
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
        };
        let mut service = PlayService::new(&root, PlayConfig::for_project(&root));
        service.worlds.insert(
            "play:1".into(),
            World {
                players: vec![(Role::Local, player)],
                scene: "fixture".into(),
                snapshot: Vec::new(),
                section: None,
                window: false,
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
            },
        );
        let began = Instant::now();
        let result = service.stop("play:1").map_err(|d| format!("{d:?}"))?;
        let elapsed = began.elapsed();
        std::fs::write(
            root.join("stop-timing.json"),
            ara_json::to_canonical_string(
                &json!({"serviceMs":elapsed.as_secs_f64()*1000.,"result":result}),
            ),
        )?;
        assert!(
            elapsed < Duration::from_millis(100),
            "stop blocked the service for {elapsed:?}"
        );
        assert!(!service.worlds.contains_key("play:1"));
        wait_file(&root.join("stop-entered"))?;
        assert_eq!(service.status()["stopping"][0]["world"], "play:1");
        for _ in 0..10 {
            assert!(service.collect_stopped().is_empty());
        }
        assert_eq!(
            service.retiring.len(),
            1,
            "pending shutdown must stay off-thread"
        );
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !service.retiring.is_empty() {
            assert!(service.collect_stopped().is_empty());
            assert!(Instant::now() < deadline, "shutdown was not reaped");
            thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}
