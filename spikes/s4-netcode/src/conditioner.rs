//! Seeded UDP impairment below netcode encryption and Renet reliability.

use serde::Serialize;
use std::{
    io,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

#[derive(Default, Serialize)]
pub struct DirectionStats {
    pub offered_packets: u64,
    pub dropped_packets: u64,
    pub offered_udp_bytes: u64,
    pub delivered_udp_bytes: u64,
    pub delay_sum_us: u64,
    pub min_delay_us: u64,
    pub max_delay_us: u64,
    pub measurement_udp_bytes: u64,
}

struct Pending {
    due: Instant,
    destination: SocketAddr,
    bytes: Vec<u8>,
    down: bool,
}

pub struct Link {
    socket: UdpSocket,
    server: Option<SocketAddr>,
    client: Option<SocketAddr>,
    queue: Vec<Pending>,
    seed: u64,
    impaired: bool,
    pub up: DirectionStats,
    pub down: DirectionStats,
}

impl Link {
    pub fn new(seed: u64, impaired: bool) -> io::Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            server: None,
            client: None,
            queue: Vec::new(),
            seed,
            impaired,
            up: DirectionStats::default(),
            down: DirectionStats::default(),
        })
    }
    pub fn address(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
    pub fn set_server(&mut self, server: SocketAddr) {
        self.server = Some(server);
    }

    /// One bounded proxy service pass. Reordering follows independently sampled delays.
    pub fn pump(&mut self, now: Instant, measure: bool) -> io::Result<()> {
        let Some(server) = self.server else {
            return Ok(());
        };
        let mut buffer = [0_u8; 4096];
        loop {
            let (len, from) = match self.socket.recv_from(&mut buffer) {
                Ok(packet) => packet,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(error) => return Err(error),
            };
            let down = from == server;
            let destination = if down {
                let Some(client) = self.client else {
                    continue;
                };
                client
            } else {
                self.client = Some(from);
                server
            };
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            let drop = self.impaired && self.seed % 10_000 < 500;
            let delay_us = if self.impaired {
                30_000 + ((self.seed >> 16) % 40_001)
            } else {
                0
            };
            let stats = if down { &mut self.down } else { &mut self.up };
            stats.offered_packets += 1;
            stats.offered_udp_bytes += len as u64;
            if measure {
                stats.measurement_udp_bytes += len as u64;
            }
            if drop {
                stats.dropped_packets += 1;
                continue;
            }
            stats.delay_sum_us += delay_us;
            stats.min_delay_us = if stats.min_delay_us == 0 {
                delay_us
            } else {
                stats.min_delay_us.min(delay_us)
            };
            stats.max_delay_us = stats.max_delay_us.max(delay_us);
            if self.queue.len() >= 8192 {
                return Err(io::Error::other(
                    "conditioner datagram queue exceeded bound",
                ));
            }
            self.queue.push(Pending {
                due: now + Duration::from_micros(delay_us),
                destination,
                bytes: buffer[..len].to_vec(),
                down,
            });
        }
        let mut index = 0;
        while index < self.queue.len() {
            if self.queue[index].due <= now {
                let packet = self.queue.swap_remove(index);
                let sent = self.socket.send_to(&packet.bytes, packet.destination)?;
                let stats = if packet.down {
                    &mut self.down
                } else {
                    &mut self.up
                };
                stats.delivered_udp_bytes += sent as u64;
            } else {
                index += 1;
            }
        }
        Ok(())
    }
}
