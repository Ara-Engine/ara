//! Seeded datagram impairment below encryption/reliability; never simulation time.
use crate::{NetError, Result};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

/// Reproducible per-direction link settings. Probabilities use basis points (0..10000).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Conditions {
    /// Added one-way latency, in milliseconds.
    pub latency_ms: u32,
    /// Uniform plus/minus one-way jitter, in milliseconds.
    pub jitter_ms: u32,
    /// Independent packet loss probability, in basis points.
    #[ara(range(min = 0, max = 10000))]
    pub loss: u16,
    /// Packet duplication probability, in basis points.
    #[ara(range(min = 0, max = 10000))]
    pub duplication: u16,
    /// Probability of an extra latency-sized delay to force reordering.
    #[ara(range(min = 0, max = 10000))]
    pub reordering: u16,
    /// Directional bandwidth in bytes/s; zero disables shaping.
    pub bandwidth: u32,
}
impl Conditions {
    /// S4 adverse profile: 100 ms nominal RTT, ±20 ms jitter, 5% directional loss.
    pub const ADVERSE: Self = Self {
        latency_ms: 50,
        jitter_ms: 20,
        loss: 500,
        duplication: 0,
        reordering: 0,
        bandwidth: 0,
    };
    /// Rejects invalid probabilities/delays before changing a live link.
    pub fn validate(self) -> Result<()> {
        if self.loss > 10000
            || self.duplication > 10000
            || self.reordering > 10000
            || self.latency_ms > 60000
            || self.jitter_ms > 60000
        {
            return Err(NetError("invalid network conditions".into()));
        }
        Ok(())
    }
}

/// Counts offered/delivered encrypted UDP payload, including transport headers and ACKs.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DirectionStats {
    /// Datagrams received from the sender.
    pub packets: u64,
    /// Datagrams dropped by configured loss or the bounded queue.
    pub dropped: u64,
    /// Duplicate datagrams scheduled.
    pub duplicated: u64,
    /// Payload bytes offered before impairment.
    pub offered_bytes: u64,
    /// Payload bytes delivered after impairment.
    pub delivered_bytes: u64,
    /// Offered payload bytes inside the caller's measurement window.
    pub measured_bytes: u64,
}
struct Packet {
    due: Instant,
    destination: SocketAddr,
    bytes: Vec<u8>,
    down: bool,
}

/// Nonblocking UDP proxy used for realistic editor sessions and process gates.
/// Packet seed/order is recorded; OS packet arrival timing is not deterministic replay.
pub struct Link {
    socket: UdpSocket,
    server: Option<SocketAddr>,
    client: Option<SocketAddr>,
    queue: Vec<Packet>,
    random_state: u64,
    initial_seed: u64,
    conditions: [Conditions; 2],
    next_send: [Instant; 2],
    /// Upstream counters.
    pub up: DirectionStats,
    /// Downstream counters.
    pub down: DirectionStats,
}
impl Link {
    /// Binds an ephemeral loopback proxy; both directions initially use `conditions`.
    pub fn new(seed: u64, conditions: Conditions) -> Result<Self> {
        conditions.validate()?;
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            server: None,
            client: None,
            queue: Vec::new(),
            random_state: seed.max(1),
            initial_seed: seed.max(1),
            conditions: [conditions; 2],
            next_send: [Instant::now(); 2],
            up: DirectionStats::default(),
            down: DirectionStats::default(),
        })
    }
    /// Recorded random seed, suitable for a session/replay artifact.
    pub fn seed(&self) -> u64 {
        self.initial_seed
    }
    /// Endpoint clients connect to through encrypted netcode tokens.
    pub fn address(&self) -> Result<SocketAddr> {
        Ok(self.socket.local_addr()?)
    }
    /// Sets the destination after the server has bound its ephemeral socket.
    pub fn set_server(&mut self, address: SocketAddr) {
        self.server = Some(address);
    }
    /// Atomically changes directional link conditions; existing packets retain their delays.
    pub fn conditions(&mut self, up: Conditions, down: Conditions) -> Result<()> {
        up.validate()?;
        down.validate()?;
        self.conditions = [up, down];
        Ok(())
    }
    fn random(&mut self) -> u64 {
        self.random_state ^= self.random_state << 13;
        self.random_state ^= self.random_state >> 7;
        self.random_state ^= self.random_state << 17;
        self.random_state
    }
    /// Services at most 1024 arrivals, then due deliveries. No worker or thread is retained.
    pub fn pump(&mut self, now: Instant, measure: bool) -> Result<()> {
        let Some(server) = self.server else {
            return Ok(());
        };
        let mut buffer = [0_u8; 4096];
        for _ in 0..1024 {
            let (len, from) = match self.socket.recv_from(&mut buffer) {
                Ok(packet) => packet,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(e) => return Err(e.into()),
            };
            let down = from == server;
            let destination = if down {
                let Some(client) = self.client else {
                    continue;
                };
                client
            } else {
                if self.client.is_some_and(|client| client != from) {
                    continue;
                }
                self.client = Some(from);
                server
            };
            let c = self.conditions[usize::from(down)];
            let loss = self.random() % 10000 < u64::from(c.loss);
            let duplicate = self.random() % 10000 < u64::from(c.duplication);
            let reorder = self.random() % 10000 < u64::from(c.reordering);
            let jitter = (self.random() % (u64::from(c.jitter_ms) * 2000 + 1)) as i64
                - i64::from(c.jitter_ms) * 1000;
            let delay = (i64::from(c.latency_ms) * 1000 + jitter).max(0) as u64
                + if reorder {
                    u64::from(c.latency_ms.max(1)) * 2000
                } else {
                    0
                };
            let stats = if down { &mut self.down } else { &mut self.up };
            stats.packets += 1;
            stats.offered_bytes += len as u64;
            if measure {
                stats.measured_bytes += len as u64;
            }
            if loss || self.queue.len() >= 8190 {
                stats.dropped += 1;
                continue;
            }
            let count = 1 + usize::from(duplicate);
            stats.duplicated += u64::from(duplicate);
            for _ in 0..count {
                let mut due = now + Duration::from_micros(delay);
                if c.bandwidth > 0 {
                    let next = &mut self.next_send[usize::from(down)];
                    due = due.max(*next);
                    *next = due + Duration::from_secs_f64(len as f64 / f64::from(c.bandwidth));
                }
                self.queue.push(Packet {
                    due,
                    destination,
                    bytes: buffer[..len].to_vec(),
                    down,
                });
            }
        }
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].due > now {
                i += 1;
                continue;
            }
            let packet = self.queue.swap_remove(i);
            let sent = self.socket.send_to(&packet.bytes, packet.destination)?;
            let stats = if packet.down {
                &mut self.down
            } else {
                &mut self.up
            };
            stats.delivered_bytes += sent as u64;
        }
        Ok(())
    }
}
