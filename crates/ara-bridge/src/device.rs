use crate::{BridgeError, Command, Error, PROTOCOL_VERSION, PerfSummary, Request, Response};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::Command as Process,
    time::Duration,
};
use tungstenite::{Message, WebSocket};

/// Device discovered through adb, including unauthorized/offline states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// adb serial (must be passed explicitly to device operations).
    pub serial: String,
    /// `device`, `offline` or `unauthorized`.
    pub state: String,
    /// Additional model/product fields from `adb devices -l`.
    pub details: Vec<String>,
}

/// adb transport. Preserves an existing server, and stops one it started on drop.
pub struct Adb {
    program: PathBuf,
    started: bool,
}
impl Adb {
    /// Creates a transport without starting an emulator or adb server.
    pub fn new(program: PathBuf) -> Self {
        let started = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], 5037)),
            Duration::from_millis(100),
        )
        .is_err();
        Self { program, started }
    }
    /// Lists devices without selecting the first device implicitly.
    pub fn list(&self) -> Result<Vec<Device>, Error> {
        let output = self.run(&["devices", "-l"])?;
        Ok(String::from_utf8_lossy(&output)
            .lines()
            .skip(1)
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                Some(Device {
                    serial: parts.next()?.into(),
                    state: parts.next()?.into(),
                    details: parts.map(str::to_owned).collect(),
                })
            })
            .collect())
    }
    /// Establishes an adb localhost TCP forward; dropping the returned handle removes it.
    pub fn forward(&self, serial: &str, device_port: u16) -> Result<AdbForward, Error> {
        let remote = format!("tcp:{device_port}");
        let output = self.run(&["-s", serial, "forward", "tcp:0", &remote])?;
        let port = String::from_utf8_lossy(&output)
            .trim()
            .parse::<u16>()
            .map_err(|_| diagnostic("adb returned no forwarded port"))?;
        Ok(AdbForward {
            program: self.program.clone(),
            serial: serial.into(),
            port,
        })
    }
    /// Pulls an absolute device file using adb's file transfer protocol.
    pub fn pull(&self, serial: &str, remote: &str, local: &Path) -> Result<(), Error> {
        if !remote.starts_with('/') || remote.contains('\0') {
            return Err(diagnostic("pull requires an absolute device path"));
        }
        let out = Process::new(&self.program)
            .args(["-s", serial, "pull", remote])
            .arg(local)
            .output()?;
        if !out.status.success() {
            return Err(diagnostic(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(())
    }
    /// Reads device logs. Runtime structured logs use DeviceClient::logs instead.
    pub fn logs(&self, serial: &str) -> Result<String, Error> {
        Ok(String::from_utf8_lossy(&self.run(&[
            "-s",
            serial,
            "logcat",
            "-d",
            "-s",
            "ara:*",
            "AndroidRuntime:E",
        ])?)
        .into_owned())
    }
    /// Captures the device display as exact PNG bytes, without shell redirection.
    pub fn screenshot(&self, serial: &str) -> Result<Vec<u8>, Error> {
        let png = self.run(&["-s", serial, "exec-out", "screencap", "-p"])?;
        if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(diagnostic("device returned no PNG"));
        }
        Ok(png)
    }
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, Error> {
        let out = Process::new(&self.program).args(args).output()?;
        if !out.status.success() {
            return Err(diagnostic(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(out.stdout)
    }
}
impl Drop for Adb {
    fn drop(&mut self) {
        if self.started {
            let _ = Process::new(&self.program).arg("kill-server").output();
        }
    }
}

/// RAII forwarding lease; keep its parent Adb transport alive until this is dropped.
pub struct AdbForward {
    program: PathBuf,
    serial: String,
    port: u16,
}
impl AdbForward {
    /// Connects a protocol client through this forward.
    pub fn connect(&self, token: String) -> Result<DeviceClient, Error> {
        DeviceClient::connect(SocketAddr::from(([127, 0, 0, 1], self.port)), token)
    }
}
impl Drop for AdbForward {
    fn drop(&mut self) {
        let _ = Process::new(&self.program)
            .args([
                "-s",
                &self.serial,
                "forward",
                "--remove",
                &format!("tcp:{}", self.port),
            ])
            .output();
    }
}

/// Bridge device API for list/logs/screenshot/pull/perf CLI integration.
pub struct DeviceClient {
    socket: WebSocket<TcpStream>,
    token: String,
    next_id: u64,
}
impl DeviceClient {
    /// Connects to a desktop bridge or adb forward with bounded network I/O.
    pub fn connect(address: SocketAddr, token: String) -> Result<Self, Error> {
        if !address.ip().is_loopback() {
            return Err(diagnostic("bridge clients require a localhost endpoint"));
        }
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let (socket, _) = tungstenite::client(format!("ws://{address}/"), stream)
            .map_err(|e| diagnostic(&e.to_string()))?;
        Ok(Self {
            socket,
            token,
            next_id: 0,
        })
    }
    /// Sends a correlated operation and checks version/ID before accepting data.
    pub fn request(&mut self, command: Command) -> Result<Value, Error> {
        self.next_id += 1;
        let req = Request {
            version: PROTOCOL_VERSION,
            id: self.next_id,
            token: self.token.clone(),
            command,
        };
        self.socket
            .send(Message::text(serde_json::to_string(&req)?))?;
        loop {
            let message = self.socket.read()?;
            if let Message::Text(text) = message {
                let response: Response = serde_json::from_str(&text)?;
                if response.version != PROTOCOL_VERSION || response.id != self.next_id {
                    return Err(diagnostic("response version or ID mismatch"));
                }
                if let Some(error) = response.error {
                    return Err(Error::Diagnostic(error));
                }
                return response.result.ok_or_else(|| diagnostic("empty response"));
            }
        }
    }
    /// Reads logs after a sequence, with ring retention metadata.
    pub fn logs(&mut self, after: u64) -> Result<Value, Error> {
        self.request(Command::Logs { after })
    }
    /// Reads frame/GPU timing histograms.
    pub fn perf(&mut self) -> Result<PerfSummary, Error> {
        Ok(serde_json::from_value(self.request(Command::Perf)?)?)
    }
    /// Captures and validates a PNG through the runtime renderer hook.
    pub fn screenshot(&mut self) -> Result<Vec<u8>, Error> {
        let data = self.request(Command::Screenshot)?;
        let encoded = data
            .get("base64")
            .and_then(Value::as_str)
            .ok_or_else(|| diagnostic("missing screenshot payload"))?;
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| diagnostic("invalid screenshot encoding"))?;
        if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(diagnostic("renderer returned no PNG"));
        }
        Ok(bytes)
    }
}

fn diagnostic(message: &str) -> Error {
    Error::Diagnostic(BridgeError::new(
        "ARA-EXP-BRG-020",
        message,
        "ara device list",
    ))
}
