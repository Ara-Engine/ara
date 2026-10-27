//! Loopback direct-connect rendezvous. Join credentials stay on this private local socket.
use anyhow::{Context, Result, ensure};
use ara_play::Joined;
use ara_runtime_net::NetHost;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

#[derive(Serialize, Deserialize)]
pub(super) struct Ticket {
    pub join: Joined,
    pub players: u64,
}

pub(super) struct Lobby {
    listener: TcpListener,
    next: usize,
    slots: usize,
}
impl Lobby {
    pub(super) fn bind(port: u16, slots: usize) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            next: 0,
            slots,
        })
    }
    pub(super) fn poll(&mut self, host: &mut NetHost) -> Result<()> {
        match self.listener.accept() {
            Ok((mut stream, address)) => {
                ensure!(
                    address.ip().is_loopback(),
                    "Outpost v0 rendezvous accepts this PC only"
                );
                stream.set_write_timeout(Some(Duration::from_millis(100)))?;
                let bytes = if self.next < self.slots {
                    let (player, endpoint, token) = host.join(self.next)?;
                    self.next += 1;
                    serde_json::to_vec(&Ticket {
                        join: Joined {
                            player,
                            endpoint: endpoint.to_string(),
                            token: ara_play::hex(&token.into_bytes()),
                        },
                        players: self.slots as u64 + 1,
                    })?
                } else {
                    serde_json::to_vec(&serde_json::json!({"error":"crew_full"}))?
                };
                stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
                stream.write_all(&bytes)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
}

pub(super) fn join(address: &str) -> Result<Ticket> {
    let address: SocketAddr = address
        .parse()
        .context("use an address such as 127.0.0.1:41090")?;
    ensure!(
        address.ip().is_loopback(),
        "Outpost v0 co-op runs on one PC; use 127.0.0.1"
    );
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut size = [0; 4];
    stream.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    ensure!(size <= 16384, "join response exceeds bounds");
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).context("host refused the join (the crew may be full)")
}

pub(super) fn players(world: &crate::World) -> String {
    let mut players: Vec<_> = world
        .runtime
        .world()
        .iter_entities()
        .filter_map(|entity| {
            let control = entity.get::<ara_runtime::components::PlayerControl>()?;
            let active = ara_runtime_script::read(world.runtime.world(), entity.id(), "Survivor")
                .is_some_and(|v| v["active"] == true);
            Some((control.player, active))
        })
        .collect();
    players.sort_unstable();
    let dedicated = players.len() > 4;
    players
        .into_iter()
        .filter(|(player, _)| !dedicated || *player > 1)
        .map(|(player, active)| {
            format!(
                "Scavenger {player}: {}",
                if active { "Ready" } else { "Waiting" }
            )
        })
        .collect::<Vec<_>>()
        .join("   |   ")
}
