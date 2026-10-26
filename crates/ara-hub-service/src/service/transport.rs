//! Authenticated Hub JSON-RPC transport and user-level auto-start.
use super::*;

pub(super) fn current_build() -> Result<String, RpcError> {
    static BUILD: std::sync::OnceLock<Result<String, String>> = std::sync::OnceLock::new();
    BUILD
        .get_or_init(|| {
            std::env::current_exe()
                .map_err(|e| e.to_string())
                .and_then(|path| ara_update::sha256_file(&path).map_err(|e| e.to_string()))
        })
        .clone()
        .map_err(io_error)
}

impl Endpoint for Hub {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.call_for(ConnId::LOCAL, method, params)
    }
}
impl Handler for Hub {
    fn message(&mut self, conn: ConnId, text: &str) -> Option<String> {
        match Incoming::parse(text) {
            Ok(Incoming::Request { id, method, params }) => {
                Some(jsonrpc::reply(id, self.call_for(conn, &method, params)).to_string())
            }
            Ok(Incoming::Notification { method, params }) => {
                let _ = self.call_for(conn, &method, params);
                None
            }
            Err(error) => Some(error.to_string()),
            _ => None,
        }
    }
    fn closed(&mut self, conn: ConnId) {
        self.routes.retain(|(c, _), _| *c != conn);
        self.subscribers.remove(&conn);
    }
    fn tick(&mut self) {
        self.poll_updates();
        self.poll_processes();
        if self.subscribers.is_empty()
            || self.processes.running.is_empty()
            || self.last_activity_check.elapsed() < Duration::from_secs(2)
        {
            return;
        }
        self.last_activity_check = Instant::now();
        let activity = self.call("hub.activity", json!({}));
        if let Ok(activity) = activity {
            let state = activity["activity"].clone();
            if self.last_status != state {
                self.last_status = state;
                self.changed("activity", None);
            }
        }
    }
    fn outbox(&mut self) -> Vec<(ConnId, String)> {
        std::mem::take(&mut self.events)
    }
    fn stopping(&self) -> bool {
        self.stop
    }
}

/// Serve one authenticated user-level Hub. The lock prevents duplicate instances.
pub fn serve(config: HubConfig, port: u16) -> Result<(), RpcError> {
    let directory = config.directory.clone();
    let mut hub = Hub::open(config)?;
    let discovery = directory.join("hub");
    let token = ara_rpc::project_token(&discovery).map_err(io_error)?;
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(io_error)?;
    DaemonInfo {
        pid: std::process::id(),
        port: listener.local_addr().map_err(io_error)?.port(),
        token: token.clone(),
        protocol: ara_proto::PROTOCOL.into(),
    }
    .write(&discovery)
    .map_err(io_error)?;
    let result = ara_rpc::serve(
        &mut hub,
        ServeOptions {
            listener: Some(listener),
            token,
            stdio: false,
            tick: Duration::from_millis(500),
        },
    )
    .map_err(io_error);
    DaemonInfo::remove(&discovery);
    result
}

/// Connect to the user Hub, auto-starting `ara hub serve` with a hidden process on demand.
pub fn connect_or_start(directory: &Path, executable: &Path) -> Result<Client, RpcError> {
    Ok(connect_checked(directory, executable)?.client)
}

/// A connected service and whether a busy older build deferred its replacement.
pub struct ConnectedService {
    /// Authenticated RPC connection.
    pub client: Client,
    /// Close projects and restart before switching to the requested service build.
    pub update_pending: bool,
}

/// Perform the build handshake off the UI thread, preserving busy older services.
pub fn connect_checked(directory: &Path, executable: &Path) -> Result<ConnectedService, RpcError> {
    crate::validate_development_config(directory).map_err(io_error)?;
    crate::isolation::validate_engine(directory, executable).map_err(io_error)?;
    let discovery = directory.join("hub");
    let phase = Instant::now();
    let build = ara_update::sha256_file(executable).map_err(io_error)?;
    crate::startup::record("client.hash", phase.elapsed());
    fs::create_dir_all(directory).map_err(io_error)?;
    // Serialize discovery/replacement/start across simultaneous Hub and MCP clients.
    // Never hold the service's own lock: it must be able to exit and start here.
    let _start_lock = startup_lock(directory)?;
    // Legacy clients don't take hub-start.lock. If one wins a simultaneous
    // startup, validate that service too, then retry the same safe replacement.
    for _ in 0..3 {
        let phase = Instant::now();
        if let Some(mut client) = Client::for_project(&discovery) {
            let status = client.call("hub.status", json!({}))?;
            crate::startup::record("client.handshake", phase.elapsed());
            if status["build"].as_str() == Some(&build) {
                return Ok(ConnectedService {
                    client,
                    update_pending: false,
                });
            }
            // Legacy previews have no build field or conditional shutdown parameter.
            // Never interrupt their supervised processes, even during an upgrade.
            if !idle_status(&status) {
                return Ok(ConnectedService {
                    client,
                    update_pending: true,
                });
            }
            let result = if status["build"].is_string() {
                client.call("hub.shutdown", json!({"onlyIfIdle":true}))?
            } else {
                client.call("hub.shutdown", json!({}))?
            };
            if result["stopping"] != true {
                return Ok(ConnectedService {
                    client,
                    update_pending: true,
                });
            }
            drop(client);
            let until = Instant::now() + Duration::from_secs(20);
            let service_lock = lock_file(&directory.join("hub.lock"))?;
            loop {
                if DaemonInfo::read(&discovery).is_none() && service_lock.try_lock().is_ok() {
                    break;
                }
                if Instant::now() >= until {
                    return Err(io_error(
                        "Hub service update timed out waiting for the previous build to stop; see hub-start.log",
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        let phase = Instant::now();
        super::background::spawn(executable, directory).map_err(io_error)?;
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(mut client) = Client::for_project(&discovery) {
                let status = client.call("hub.status", json!({}))?;
                if status["build"].as_str() != Some(&build) {
                    break;
                }
                crate::startup::record("client.spawn_ready", phase.elapsed());
                return Ok(ConnectedService {
                    client,
                    update_pending: false,
                });
            }
            if Instant::now() >= until {
                return Err(io_error("Hub startup timed out; see hub-start.log"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    Err(io_error(
        "A different Hub service build repeatedly won startup; close older Hub windows and restart; see hub-start.log",
    ))
}

fn idle_status(status: &Value) -> bool {
    status["projects"]
        .as_array()
        .is_some_and(|projects| projects.iter().all(|project| project["state"] == "closed"))
}

pub(super) fn lock_file(path: &Path) -> Result<File, RpcError> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io_error)
}

pub(super) fn startup_lock(directory: &Path) -> Result<File, RpcError> {
    let lock = lock_file(&directory.join("hub-start.lock"))?;
    let until = Instant::now() + Duration::from_secs(20);
    while lock.try_lock().is_err() {
        if Instant::now() >= until {
            return Err(io_error(
                "Hub startup lock timed out; another client is updating the service; see hub-start.log",
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(lock)
}
