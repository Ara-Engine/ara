//! Authenticated Hub JSON-RPC transport and user-level auto-start.
use super::*;

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
        for child in self.children.values_mut() {
            let _ = child.try_wait();
        }
        if self.subscribers.is_empty() {
            return;
        }
        let status = self.call("hub.status", json!({}));
        let activity = self.call("hub.activity", json!({}));
        if let (Ok(status), Ok(activity)) = (status, activity) {
            let state = json!({"projects":status["projects"],"activity":activity["activity"]});
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
    let discovery = directory.join("hub");
    if let Some(client) = Client::for_project(&discovery) {
        return Ok(client);
    }
    fs::create_dir_all(directory).map_err(io_error)?;
    super::background::spawn(executable, directory).map_err(io_error)?;
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(client) = Client::for_project(&discovery) {
            return Ok(client);
        }
        if Instant::now() >= until {
            return Err(io_error("Hub startup timed out; see hub-start.log"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
