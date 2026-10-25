//! Small authenticated lifetime channel; codec traffic remains in the GPU crate.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

/// Private loopback endpoint of a shared optional encoder, never a cache input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncoderEndpoint {
    /// Loopback address allocated by the encoder service.
    pub address: SocketAddr,
    /// Random per-process authentication token inherited only by trusted workers.
    pub token: [u8; 32],
}
impl EncoderEndpoint {
    /// Opens an authenticated encoding channel without extending host lifetime.
    pub fn connect(&self) -> Result<TcpStream> {
        self.open(1)
    }

    /// Keeps the service alive until this connection closes, including host exit.
    /// The host never loads Vulkan or executes importer code through this lease.
    pub fn lease(&self) -> Result<TcpStream> {
        self.open(0)
    }

    fn open(&self, role: u8) -> Result<TcpStream> {
        if !self.address.ip().is_loopback() {
            return Err(Error::EncoderUnavailable(
                "encoder endpoint is not loopback".into(),
            ));
        }
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(2))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        stream.write_all(b"ARAENC01")?;
        stream.write_all(&self.token)?;
        stream.write_all(&[role])?;
        let mut ack = [0xff];
        stream.read_exact(&mut ack)?;
        if ack != [0] {
            return Err(Error::EncoderUnavailable(
                "encoder service rejected authentication".into(),
            ));
        }
        Ok(stream)
    }
}
