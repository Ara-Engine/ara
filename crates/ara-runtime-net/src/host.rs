//! The play world as a networked authority (listen host or dedicated server).
use super::*;
use ara_input::Injection;
use ara_net::{
    AdapterRef, Authoritative, Conditions, JoinToken, Link, PolicyRegistry, Server, ServerConfig,
};
use ara_sim::{SessionAdapter, SessionMode, TickRecord};
use std::{net::SocketAddr, time::Instant};

/// The runtime as the server's authority: its plugins run around every boundary.
struct Driver(Runtime);

impl Authoritative for Driver {
    fn session(&self) -> &Session {
        self.0.play().session()
    }
    fn session_mut(&mut self) -> &mut Session {
        self.0.play_mut().session_mut()
    }
    fn step(
        &mut self,
        adapter: &mut dyn SessionAdapter,
    ) -> std::result::Result<TickRecord, SimError> {
        self.0
            .step_with_adapter(&mut AdapterRef(adapter))
            .map_err(|e| SimError(e.0))
    }
}

/// How a networked authority starts.
#[derive(Debug, Clone, Default)]
pub struct HostConfig {
    /// A renderer-free authority without a local player; otherwise a listen host that
    /// plays as player 1.
    pub dedicated: bool,
    /// One entry per client (players 2, 3, ...): simulated link conditions, if any.
    /// Every client joins through its own `Link` proxy, which also measures traffic.
    pub clients: Vec<Option<Conditions>>,
}

/// Authority-side numbers for reports and `play.status`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStats {
    /// Simulation tick.
    pub tick: u64,
    /// Ready remote clients.
    pub connections: usize,
    /// Host creation until the first networked boundary (every client joined), in ms.
    pub begin_ms: Option<f64>,
    /// Replication payload offered by the field codecs, in bytes.
    pub payload_bytes: u64,
    /// Encrypted datagram bytes sent to clients (headers, ACKs and control included).
    pub down_bytes: u64,
    /// Encrypted datagram bytes received from clients.
    pub up_bytes: u64,
    /// Datagrams sent to clients.
    pub down_packets: u64,
    /// Datagrams dropped by simulated loss, both directions.
    pub dropped: u64,
}

/// The play world's [`Runtime`] owned by a Lightyear server. The server's timeline
/// steps it (call [`NetHost::poll`] every frame); reach it between polls with
/// [`NetHost::runtime`] and [`NetHost::runtime_mut`].
pub struct NetHost {
    server: Server<Locomotion>,
    links: Vec<Link>,
    players: Vec<(String, u64)>,
    dedicated: bool,
    created: Instant,
    begin_ms: Option<f64>,
}

impl NetHost {
    /// Wraps a freshly built world (tick 0, after [`avatars`], with [`NetPlugin`]):
    /// attaches motors, registers every player avatar with its owner and binds one
    /// proxy per client. Simulation starts once every client is ready.
    pub fn new(mut runtime: Runtime, config: HostConfig) -> Result<Self> {
        let players = attach_motors(&mut runtime);
        let hz = runtime.hz();
        let policies = PolicyRegistry::new(runtime.play().session().schema().clone())?;
        let version = policies.hash();
        let mut links = Vec::new();
        for (index, conditions) in config.clients.iter().enumerate() {
            links.push(Link::new(
                0x0a7a + index as u64,
                conditions.unwrap_or_default(),
            )?);
        }
        let expected = links
            .iter()
            .map(Link::address)
            .collect::<Result<Vec<_>>>()?;
        let mut server = Server::<Locomotion>::new(
            Driver(runtime),
            policies,
            ServerConfig {
                mode: if config.dedicated {
                    SessionMode::Dedicated
                } else {
                    SessionMode::Listen
                },
                local_owner: PlayerId(HOST_PLAYER),
                max_clients: config.clients.len().max(1),
                start_clients: config.clients.len(),
                hz,
                snapshot_hz: hz.min(30),
                expected_addresses: expected,
                ..Default::default()
            },
        )?;
        for (identity, player) in &players {
            server.spawn(
                identity,
                &format!("@play#{identity}"),
                version,
                Some(PlayerId(*player)),
                None,
            )?;
        }
        let address = server.address()?;
        for link in &mut links {
            link.set_server(address);
        }
        Ok(Self {
            server,
            links,
            players,
            dedicated: config.dedicated,
            created: Instant::now(),
            begin_ms: None,
        })
    }

    /// The join credential of client `index` (player `index + 2`), bound to its proxy:
    /// `(player, endpoint, token)`. Private launch data: hand it to that client only.
    pub fn join(&mut self, index: usize) -> Result<(u64, SocketAddr, JoinToken)> {
        let endpoint = self
            .links
            .get(index)
            .ok_or_else(|| NetError(format!("no client {index}")))?
            .address()?;
        let player = index as u64 + 2;
        Ok((
            player,
            endpoint,
            self.server.token(PlayerId(player), endpoint)?,
        ))
    }

    /// Pumps the client proxies and polls the server, which steps the runtime on its
    /// own fixed timeline (bounded catch-up).
    pub fn poll(&mut self) -> Result<()> {
        for link in &mut self.links {
            link.pump(Instant::now(), true)?;
        }
        self.server.poll()?;
        for link in &mut self.links {
            link.pump(Instant::now(), true)?;
        }
        if self.begin_ms.is_none() && self.server.session().tick() > 0 {
            self.begin_ms = Some(self.created.elapsed().as_secs_f64() * 1000.0);
        }
        Ok(())
    }

    /// The listen host's own input (player 1), `after` ticks past the next boundary:
    /// `Move` through the server's host path, other actions straight into the session.
    pub fn input(&mut self, after: u64, inputs: Vec<Injection>) -> Result<()> {
        let tick = self.server.session().tick() + 1 + after;
        let (moves, rest): (Vec<_>, Vec<_>) = inputs.into_iter().partition(|i| i.action == MOVE);
        for input in moves {
            self.server.inject_host(tick, input)?;
        }
        if !rest.is_empty() {
            self.runtime_mut()
                .play_mut()
                .session_mut()
                .inject(tick, rest)?;
        }
        Ok(())
    }

    /// True for a dedicated authority (no local player).
    pub fn dedicated(&self) -> bool {
        self.dedicated
    }

    /// The play world (read between polls).
    pub fn runtime(&self) -> &Runtime {
        &self
            .server
            .driver::<Driver>()
            .expect("NetHost always drives its server with a Driver")
            .0
    }

    /// The play world, mutably, between polls (captures, inspection, host input).
    pub fn runtime_mut(&mut self) -> &mut Runtime {
        &mut self
            .server
            .driver_mut::<Driver>()
            .expect("NetHost always drives its server with a Driver")
            .0
    }

    /// Player avatars and their owners.
    pub fn players(&self) -> &[(String, u64)] {
        &self.players
    }

    /// Changes one client's simulated conditions (both directions).
    pub fn conditions(&mut self, index: usize, conditions: Conditions) -> Result<()> {
        self.links
            .get_mut(index)
            .ok_or_else(|| NetError(format!("no client {index}")))?
            .conditions(conditions, conditions)
    }

    /// Traffic and timeline numbers so far.
    pub fn stats(&self) -> HostStats {
        let mut stats = HostStats {
            tick: self.server.session().tick(),
            connections: self.server.connections(),
            begin_ms: self.begin_ms,
            payload_bytes: self.server.profile().values().map(|f| f.bytes).sum(),
            ..Default::default()
        };
        for link in &self.links {
            stats.down_bytes += link.down.delivered_bytes;
            stats.up_bytes += link.up.delivered_bytes;
            stats.down_packets += link.down.packets;
            stats.dropped += link.down.dropped + link.up.dropped;
        }
        stats
    }
}
