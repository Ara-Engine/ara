//! One OS process, independently owned player links/worlds and shared lifetime.
mod child;
pub use child::ChildHandle;

use crate::pool::{Player, PoolConfig};
use ara_play::{
    HELLO, Hello, PROTOCOL, TOKEN_ENV,
    link::{Link, Message, error},
};
use serde_json::{Value, json};
use std::{
    net::TcpListener,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) struct Request {
    pub name: String,
    pub count: usize,
    pub timeout: Duration,
}
pub(crate) fn spawn(
    config: &PoolConfig,
    token: &str,
    scripts: Option<&ara_play::ScriptScene>,
    rendering: ara_play::RenderSettings,
    request: Request,
    mut check: impl FnMut() -> Result<(), &'static str>,
) -> Result<Vec<Player>, String> {
    let Request {
        name,
        count,
        timeout,
    } = request;
    if !(1..=17).contains(&count) {
        return Err("Shared Play needs 1–17 player worlds".into());
    }
    check().map_err(str::to_owned)?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let log = config
        .logs
        .join(format!("player-{}-{name}.log", std::process::id()));
    let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let mut command = Command::new(&config.executable);
    command
        .arg("--owner")
        .arg(
            listener
                .local_addr()
                .map_err(|e| e.to_string())?
                .port()
                .to_string(),
        )
        .arg("--name")
        .arg(&name)
        .arg("--group-slots")
        .arg(count.to_string())
        .arg("--gpu-cache")
        .arg(&config.gpu_cache)
        .arg("--size")
        .arg(format!("{}x{}", config.size[0], config.size[1]))
        .arg(format!("--ray-tracing={}", rendering.ray_tracing))
        .env(TOKEN_ENV, token)
        .env("ARA_CRASH_DIR", config.logs.join("crashes"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(file));
    if let Some(scripts) = scripts {
        command.env(
            "ARA_PLAY_SCRIPTS",
            serde_json::to_string(scripts).map_err(|e| e.to_string())?,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let child = command.spawn().map_err(|e| e.to_string())?;
    let mut handles = ChildHandle::share(child, count).map_err(|e| e.to_string())?;
    // A partially authenticated group must never leak its owned child.
    let result = (|| {
        let began = Instant::now();
        let mut players = Vec::new();
        while players.len() < count {
            check().map_err(str::to_owned)?;
            if began.elapsed() > timeout {
                return Err(format!(
                    "Shared player group did not become ready within {timeout:?}"
                ));
            }
            if let Some(handle) = handles.first_mut()
                && handle.try_wait().map_err(|e| e.to_string())?.is_some()
            {
                return Err(format!(
                    "Shared player exited before every world connected: {}",
                    log.display()
                ));
            }
            let (stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(e) => return Err(e.to_string()),
            };
            let mut link = Link::accept(stream).map_err(|e| e.to_string())?;
            let message = link
                .wait(Duration::from_secs(5))
                .map_err(|e| e.to_string())?
                .ok_or("Shared player did not send hello")?;
            let Message::Request { id, method, params } = message else {
                return Err("Shared player must authenticate with hello".into());
            };
            let hello: Hello = serde_json::from_value(params).map_err(|e| e.to_string())?;
            let slot = hello
                .name
                .strip_prefix(&format!("{name}-"))
                .and_then(|s| s.parse::<usize>().ok());
            let expected_pid = handles
                .first()
                .map(ChildHandle::id)
                .or_else(|| players.first().map(|p: &Player| p.child.id()));
            if method != HELLO
                || hello.token != token
                || hello.protocol != PROTOCOL
                || Some(hello.pid) != expected_pid
                || hello.window
                || !slot.is_some_and(|slot| slot < count)
                || players.iter().any(|p: &Player| p.hello.name == hello.name)
            {
                let _ = link.respond(
                    id,
                    Err(error(
                        -32001,
                        "Unauthorized or incompatible shared player",
                        Value::Null,
                    )),
                );
                return Err("Rejected invalid shared player authentication".into());
            }
            link.respond(id, Ok(json!({})))
                .and_then(|_| link.flush())
                .map_err(|e| e.to_string())?;
            players.push(Player {
                child: handles.pop().ok_or("Shared player handles exhausted")?,
                link,
                hello,
                log: log.clone(),
                seen: Instant::now(),
                status: None,
                grace: Instant::now(),
                events: Vec::new(),
                embed: Vec::new(),
                replies: Default::default(),
            });
        }
        players.sort_by_key(|p| {
            p.hello
                .name
                .rsplit_once('-')
                .and_then(|(_, s)| s.parse::<usize>().ok())
                .unwrap_or(0)
        });
        // launch() pops in role order, so leave slot zero last.
        players.reverse();
        Ok(players)
    })();
    if result.is_err()
        && let Some(handle) = handles.first_mut()
    {
        let _ = handle.kill();
        let _ = handle.wait();
    }
    result
}
