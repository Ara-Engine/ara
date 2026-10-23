//! S5's pre-spawned single-use process pattern. No project/worktree clones.
use crate::{NetError, Result};
use ara_sim::SessionMode;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// Role assigned to a warm player only when Play begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerRole {
    /// Player-hosted authority, with direct local-owner input.
    Host,
    /// Renderer-free authority.
    Dedicated,
    /// Remote player/bot.
    Client,
}
/// Editor-owned process topology; each viewport can bind to one returned child.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    /// Listen or Dedicated authority.
    pub mode: SessionMode,
    /// Number of additional remote player processes.
    pub clients: usize,
}
impl LaunchPlan {
    /// Stable role order: authority first, then clients in player order.
    pub fn roles(&self) -> Result<Vec<PlayerRole>> {
        if self.clients > 16 || self.mode == SessionMode::Local {
            return Err(NetError(
                "process launch requires Host/Dedicated plus at most 16 clients".into(),
            ));
        }
        let authority = if self.mode == SessionMode::Listen {
            PlayerRole::Host
        } else {
            PlayerRole::Dedicated
        };
        let mut roles = vec![authority];
        roles.extend(std::iter::repeat_n(PlayerRole::Client, self.clients));
        Ok(roles)
    }
}
struct Player {
    child: OwnedChild,
    stream: TcpStream,
}
struct OwnedChild {
    process: Child,
    log: PathBuf,
}
impl std::ops::Deref for OwnedChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.process
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.process
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.process.try_wait(), Ok(Some(_))) {
            let _ = self.process.kill();
        }
        let _ = self.process.wait();
    }
}
impl OwnedChild {
    fn diagnostics(&self) -> String {
        let tail = (|| -> std::io::Result<String> {
            let mut file = std::fs::File::open(&self.log)?;
            let len = file.metadata()?.len();
            file.seek(SeekFrom::Start(len.saturating_sub(4096)))?;
            let mut bytes = Vec::new();
            file.take(4096).read_to_end(&mut bytes)?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })();
        format!("{}: {}", self.log.display(), tail.unwrap_or_default())
    }
}
/// Warm single-use child processes. Construct/refill away from the editor tick thread.
pub struct PlayerPool {
    players: Vec<Player>,
}
impl PlayerPool {
    /// Pre-spawns workers using their ordinary executable plus caller-selected args.
    /// The executable calls `player_worker` when ARA_NET_POOL_ADDR is present.
    pub fn warm(executable: &Path, args: &[String], count: usize) -> Result<Self> {
        if count == 0 || count > 17 {
            return Err(NetError("pool size must be 1..17".into()));
        }
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let mut pool = Self {
            players: Vec::new(),
        };
        for _ in 0..count {
            let key = lightyear::netcode::generate_key();
            let nonce = key.iter().map(|b| format!("{b:02x}")).collect::<String>();
            let mut command = Command::new(executable);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(crate::error)?
                .as_nanos();
            let log = std::env::temp_dir()
                .join(format!("ara-net-player-{}-{stamp}.log", std::process::id()));
            let stderr = std::fs::File::create(&log)?;
            command
                .args(args)
                .env("ARA_NET_POOL_ADDR", listener.local_addr()?.to_string())
                .env("ARA_NET_POOL_TOKEN", &nonce)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(stderr));
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000 | 0x0000_4000);
            }
            let mut child = OwnedChild {
                process: command.spawn()?,
                log,
            };
            let start = Instant::now();
            let mut rejected_handshake = String::new();
            let stream = loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Winsock inherits the listener's nonblocking mode. The warm
                        // handshake waits for the complete token under a deadline.
                        stream.set_nonblocking(false)?;
                        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
                        let mut token = vec![0; nonce.len()];
                        match stream.read_exact(&mut token) {
                            Ok(()) if token == nonce.as_bytes() => {
                                stream.set_nodelay(true)?;
                                stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                                break stream;
                            }
                            Ok(()) => rejected_handshake = "invalid readiness token".into(),
                            Err(e) => {
                                rejected_handshake = format!(
                                    "readiness read failed after {:?}: {e}",
                                    start.elapsed()
                                )
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(e.into());
                    }
                }
                if let Some(status) = child.try_wait()? {
                    return Err(NetError(format!(
                        "warm player exited {status}; {rejected_handshake}; {}",
                        child.diagnostics()
                    )));
                }
                if start.elapsed() > Duration::from_secs(10) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(NetError(format!(
                        "warm player readiness deadline exceeded; {}",
                        child.diagnostics()
                    )));
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            pool.players.push(Player { child, stream });
        }
        Ok(pool)
    }
    /// Assigns roles and trusted binary start payloads without spawning new processes.
    /// Payloads contain cooked scene/baseline handles and private join credentials;
    /// the player executable decides their versioned format.
    pub fn launch(&mut self, plan: &LaunchPlan, payloads: Vec<Vec<u8>>) -> Result<RunningPlayers> {
        let roles = plan.roles()?;
        if payloads.len() != roles.len()
            || self.players.len() < roles.len()
            || payloads.iter().any(|p| p.len() > 64 * 1024 * 1024)
        {
            return Err(NetError("pool/payload count or size mismatch".into()));
        }
        let mut running = RunningPlayers {
            players: Vec::new(),
        };
        for (role, payload) in roles.into_iter().zip(payloads) {
            let mut player = self.players.pop().expect("pool capacity checked");
            let packet = postcard::to_allocvec(&(role, payload))?;
            player
                .stream
                .write_all(&(packet.len() as u32).to_le_bytes())?;
            player.stream.write_all(&packet)?;
            running.players.push(player);
        }
        Ok(running)
    }
    /// Number of ready single-use workers left in the pool.
    pub fn available(&self) -> usize {
        self.players.len()
    }
}
/// Owned player processes; Drop/stop kills and reaps every authority/client.
pub struct RunningPlayers {
    players: Vec<Player>,
}
impl RunningPlayers {
    /// OS process IDs for editor viewport/crash reporting integration.
    pub fn ids(&self) -> Vec<u32> {
        self.players.iter().map(|p| p.child.id()).collect()
    }
    /// Polls exit status; a failed child is reported without blocking the host.
    pub fn finished(&mut self) -> Result<bool> {
        let mut done = true;
        for p in &mut self.players {
            match p.child.try_wait()? {
                Some(s) if !s.success() => {
                    return Err(NetError(format!(
                        "player {} exited {s}; {}",
                        p.child.id(),
                        p.child.diagnostics()
                    )));
                }
                Some(_) => {}
                None => done = false,
            }
        }
        Ok(done)
    }
    /// Explicitly stops/reaps all owned processes; also happens on Drop.
    pub fn stop(self) {
        drop(self)
    }
}
/// Worker entry point for `ara-player` or a headless test executable. The callback
/// runs one assigned role, then the process exits; it never returns to the pool.
pub fn player_worker(run: impl FnOnce(PlayerRole, Vec<u8>) -> Result<()>) -> Result<()> {
    let address = std::env::var("ARA_NET_POOL_ADDR").map_err(crate::error)?;
    let token = std::env::var("ARA_NET_POOL_TOKEN").map_err(crate::error)?;
    let mut stream = TcpStream::connect(address)?;
    stream.set_nodelay(true)?;
    stream.write_all(token.as_bytes())?;
    let mut size = [0; 4];
    stream.read_exact(&mut size)?;
    let length = u32::from_le_bytes(size) as usize;
    if length > 64 * 1024 * 1024 {
        return Err(NetError("player start payload exceeds limit".into()));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    let (role, payload) = postcard::from_bytes(&bytes)?;
    run(role, payload)
}
