//! Networked roles (D19). A host or dedicated authority's runtime lives inside the
//! ara-net server, which is its clock; a client presents a never-stepped mirror of the
//! authority's world. [`Play`] derefs to the runtime in every role, so the `play.*`
//! read, inspect and capture paths are role-blind.
use anyhow::{Context, Result, anyhow, bail};
use ara_input::{Injection, PlayerId};
use ara_play::{LinkConditions, NetStart, Role};
use ara_runtime::Runtime;
use ara_runtime_net::{Conditions, HostConfig, JoinToken, NetHost, NetMirror};
use ara_sim::FrameAdvance;
use serde_json::Value;
use std::{
    ops::{Deref, DerefMut},
    time::Duration,
};

/// The world's runtime in its role.
pub enum Play {
    /// Local play: stepped here by host time.
    Local(Runtime),
    /// Host or dedicated authority: the server owns the runtime and steps it.
    Host(Box<NetHost>),
    /// A client: the mirror world and its connection.
    Mirror(Runtime, Box<NetMirror>),
}

impl Deref for Play {
    type Target = Runtime;
    fn deref(&self) -> &Runtime {
        match self {
            Play::Local(runtime) | Play::Mirror(runtime, _) => runtime,
            Play::Host(host) => host.runtime(),
        }
    }
}

impl DerefMut for Play {
    fn deref_mut(&mut self) -> &mut Runtime {
        match self {
            Play::Local(runtime) | Play::Mirror(runtime, _) => runtime,
            Play::Host(host) => host.runtime_mut(),
        }
    }
}

fn conditions(c: LinkConditions) -> Conditions {
    Conditions {
        latency_ms: c.latency_ms,
        jitter_ms: c.jitter_ms,
        loss: c.loss,
        duplication: c.duplication,
        reordering: c.reordering,
        bandwidth: c.bandwidth,
    }
}

impl Play {
    /// Puts a freshly built runtime in its role: authorities wrap it in a server,
    /// clients connect with their join (connect, schema check, timeline sync).
    pub fn new(runtime: Runtime, role: Role, net: Option<&NetStart>) -> Result<Self> {
        let Some(net) = net else {
            return Ok(Play::Local(runtime));
        };
        Ok(match role {
            Role::Local => Play::Local(runtime),
            Role::Host | Role::Dedicated => Play::Host(Box::new(NetHost::new(
                runtime,
                HostConfig {
                    dedicated: role == Role::Dedicated,
                    clients: net.clients.iter().map(|c| c.map(conditions)).collect(),
                },
            )?)),
            Role::Client => {
                let mut runtime = runtime;
                let join = net.join.as_ref().context("a client needs its join")?;
                let token = ara_play::unhex_bytes(&join.token).map_err(|e| anyhow!(e))?;
                let mirror = NetMirror::connect(
                    &mut runtime,
                    join.player,
                    join.endpoint.parse()?,
                    JoinToken::from_bytes(token)?,
                )?;
                Play::Mirror(runtime, Box::new(mirror))
            }
        })
    }

    /// True for host, dedicated and client roles.
    pub fn networked(&self) -> bool {
        !matches!(self, Play::Local(_))
    }

    /// The native client clock must synchronize before its lobby accepts input.
    pub fn synced(&mut self) -> bool {
        match self {
            Play::Mirror(_, mirror) => mirror.synced(),
            _ => true,
        }
    }

    /// Fails for methods that move simulation time: networked worlds run on the
    /// authority's timeline.
    pub fn local_only(&self, what: &str) -> Result<()> {
        if self.networked() {
            bail!(
                "{what}: networked worlds run on the authority's timeline (use play.input and play.read)"
            );
        }
        Ok(())
    }

    /// One frame: local worlds advance by host time while running; authorities poll
    /// their server (which steps the world); clients poll and present their mirror.
    pub fn advance(&mut self, elapsed: Duration) -> Result<Option<FrameAdvance>> {
        match self {
            Play::Local(runtime) if runtime.running() => Ok(Some(runtime.advance(elapsed)?)),
            Play::Local(_) => Ok(None),
            Play::Host(host) => {
                host.poll()?;
                Ok(None)
            }
            Play::Mirror(runtime, mirror) => {
                mirror.frame(runtime)?;
                Ok(None)
            }
        }
    }

    /// Clients: polls until the timeline synced with the authority (the join is
    /// complete), at most `deadline`.
    pub fn connect(&mut self, deadline: Duration) -> Result<()> {
        let Play::Mirror(runtime, mirror) = self else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        while !mirror.synced() {
            if started.elapsed() > deadline {
                bail!("the authority did not accept this client within {deadline:?}");
            }
            mirror.frame(runtime)?;
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    /// Input for this process's player, `after` ticks past the next one: the listen
    /// host's player 1, a client's own player; dedicated authorities have none.
    /// Other players' inputs are dropped (their own processes predict them).
    pub fn input(&mut self, after: u64, inputs: Vec<Injection>) -> Result<()> {
        match self {
            Play::Local(runtime) => {
                let tick = runtime.tick() + 1 + after;
                runtime.play_mut().session_mut().inject(tick, inputs)?;
            }
            Play::Host(host) => {
                if host.dedicated() {
                    return Ok(());
                }
                let mine: Vec<_> = inputs
                    .into_iter()
                    .filter(|i| i.player == PlayerId(ara_runtime_net::HOST_PLAYER))
                    .collect();
                host.input(after, mine)?;
            }
            Play::Mirror(_, mirror) => {
                let player = mirror.player();
                let mine: Vec<_> = inputs.into_iter().filter(|i| i.player == player).collect();
                mirror.input(after, mine)?;
            }
        }
        Ok(())
    }

    /// The player whose devices drive this process.
    pub fn player(&self) -> PlayerId {
        match self {
            Play::Mirror(_, mirror) => mirror.player(),
            _ => PlayerId(ara_runtime_net::HOST_PLAYER),
        }
    }

    /// Authority traffic or client join/prediction numbers (status heartbeats).
    pub fn stats(&mut self) -> Option<Value> {
        match self {
            Play::Local(_) => None,
            Play::Host(host) => serde_json::to_value(host.stats()).ok(),
            Play::Mirror(_, mirror) => serde_json::to_value(mirror.stats()).ok(),
        }
    }

    /// Authorities: client `index`'s join (`play.join`).
    pub fn join(&mut self, index: usize) -> Result<ara_play::Joined> {
        let Play::Host(host) = self else {
            bail!("only a networked authority issues joins");
        };
        let (player, endpoint, token) = host.join(index)?;
        Ok(ara_play::Joined {
            player,
            endpoint: endpoint.to_string(),
            token: ara_play::hex(&token.into_bytes()),
        })
    }
}
