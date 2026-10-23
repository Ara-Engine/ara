//! The engine behind protocol commands (`ara do`, `query`, `undo`, ...): the
//! project's daemon when one answers, otherwise an in-process host. Either way the
//! command runs in its own session, which ends when the command does.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use ara_diag::{Diagnostic, codes};
use ara_host::{Host, HostConfig};
use ara_proto::{Endpoint, RpcError};
use ara_rpc::Client;
use serde_json::{Value, json};

/// How long an offline command waits for a busy project lock (another command, or
/// a daemon that is still starting).
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// A session on the project's engine.
pub struct Engine {
    ep: Box<dyn Endpoint>,
    session: String,
}

/// The daemon if one answers, else an in-process host (waiting while another
/// process holds the lock, and switching to a daemon as soon as one answers).
pub fn endpoint(root: &Path) -> Result<Box<dyn Endpoint>, Diagnostic> {
    let start = Instant::now();
    loop {
        if let Some(c) = Client::for_project(root) {
            return Ok(Box::new(c));
        }
        match Host::open(crate::project::load(root)?, HostConfig::default()) {
            Ok(h) => return Ok(Box::new(h)),
            Err(d) if d.code == codes::DAEMON_RUNNING && start.elapsed() < LOCK_WAIT => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(d) => return Err(d),
        }
    }
}

impl Engine {
    /// Connects and begins a session attributed to `author`.
    pub fn connect(root: &Path, author: &str, label: &str) -> Result<Engine, Vec<Diagnostic>> {
        let mut ep = endpoint(root).map_err(|d| vec![d])?;
        let begun = ep
            .call(
                "session.begin",
                json!({ "client": "ara-cli", "author": author, "label": label }),
            )
            .map_err(|e| e.diagnostics())?;
        let session = begun["session"].as_str().unwrap_or_default().to_owned();
        Ok(Engine { ep, session })
    }

    /// Calls `method` with `params` plus this session.
    pub fn call(&mut self, method: &str, mut params: Value) -> Result<Value, RpcError> {
        if let Some(o) = params.as_object_mut() {
            o.insert("session".into(), self.session.clone().into());
        }
        self.ep.call(method, params)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self
            .ep
            .call("session.end", json!({ "session": self.session }));
    }
}

/// Prints a method result: canonical JSON with `--json`, else its `text` or the
/// given summary. Returns the exit status (true = success).
pub fn print(
    result: Result<Value, RpcError>,
    json: bool,
    summary: impl Fn(&Value) -> String,
) -> bool {
    match result {
        Ok(v) => {
            if json {
                print!("{}", ara_json::to_canonical_string(&v));
            } else {
                match v.get("text").and_then(Value::as_str) {
                    Some(t) => print!("{t}"),
                    None => println!("{}", summary(&v)),
                }
            }
            true
        }
        Err(e) => {
            print_diags(&e.diagnostics(), json);
            false
        }
    }
}

/// Prints diagnostics as text blocks or as `{ "ok": false, "diagnostics": [...] }`.
pub fn print_diags(diags: &[Diagnostic], json: bool) {
    if json {
        print!(
            "{}",
            ara_json::to_canonical_string(&json!({ "ok": false, "diagnostics": diags }))
        );
    } else {
        for d in diags {
            eprintln!("{d}\n");
        }
    }
}
