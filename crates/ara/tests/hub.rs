//! Real global MCP -> user Hub -> two independent project daemon processes.
#[path = "../../ara-hub-service/tests/config_isolation.rs"]
mod isolation;
#[path = "../../ara-hub-service/tests/support/mod.rs"]
mod support;
use ara_proto::Endpoint;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, channel},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct TestHub {
    directory: PathBuf,
}

fn cli_executable() -> PathBuf {
    let path = fs::canonicalize(env!("CARGO_BIN_EXE_ara")).expect("CLI executable");
    ara_hub_service::normalized_path(&path).to_owned()
}

impl TestHub {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/hub-tests")
            .join(format!("{}-{name}-{unique}", std::process::id()));
        fs::create_dir_all(&directory).expect("scratch");
        Self { directory }
    }
    fn config(&self) -> PathBuf {
        self.directory.join("config")
    }
    fn cli(&self, args: &[&str]) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_ara"))
            .env("ARA_CONFIG_DIR", self.config())
            .args(args)
            .output()
            .expect("ara");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("JSON")
    }
    fn create(&self, name: &str) -> Value {
        self.cli(&[
            "hub",
            "create",
            self.directory.join(name).to_str().expect("path"),
            "--name",
            name,
        ])
    }
    fn client(&self) -> ara_rpc::Client {
        ara_rpc::Client::for_project(&self.config().join("hub")).expect("Hub running")
    }
}
impl Drop for TestHub {
    fn drop(&mut self) {
        if let Err(error) = support::shutdown(&self.config()) {
            if thread::panicking() {
                eprintln!("{error}");
            } else {
                panic!("{error}");
            }
        }
    }
}

#[test]
fn create_makes_parents_and_reports_the_original_service_diagnostic() {
    let hub = TestHub::new("create-parents");
    let path = hub.directory.join("missing/nested/Project");
    hub.cli(&[
        "hub",
        "create",
        path.to_str().expect("path"),
        "--name",
        "Nested",
    ]);
    assert!(path.join("ara.project.json").is_file());
    for json in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ara"));
        command.env("ARA_CONFIG_DIR", hub.config());
        if json {
            command.arg("--json");
        }
        let output = command
            .args([
                "hub",
                "create",
                path.to_str().expect("path"),
                "--name",
                "Another",
            ])
            .output()
            .expect("ara");
        assert!(!output.status.success());
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains("ARA1003"), "original service error: {text}");
        assert!(
            text.contains("ARA1602"),
            "catalogued invalid params: {text}"
        );
        if json {
            let result: Value = serde_json::from_slice(&output.stdout).expect("JSON diagnostic");
            assert_eq!(result["diagnostics"][0]["location"]["path"], "path");
        } else {
            assert_eq!(text.matches("error[").count(), 1, "{text}");
        }
    }
}

#[test]
fn installation_registration_does_not_start_a_service_and_reuses_one_when_running() {
    let hub = TestHub::new("no-autostart");
    hub.cli(&["hub", "register-engine", "--no-start"]);
    assert!(ara_rpc::DaemonInfo::read(&hub.config().join("hub")).is_none());
    let registry = ara_hub_service::Registry::load(&hub.config()).expect("registry");
    assert_eq!(registry.engines.len(), 1);
    assert_eq!(registry.engines[0].executable, cli_executable());
    hub.cli(&["hub", "unregister-engine", "--no-start"]);
    assert!(
        ara_hub_service::Registry::load(&hub.config())
            .expect("registry")
            .engines
            .is_empty()
    );
    assert!(ara_rpc::DaemonInfo::read(&hub.config().join("hub")).is_none());
    let alternate = hub
        .directory
        .join(format!("alternate{}", std::env::consts::EXE_SUFFIX));
    fs::copy(env!("CARGO_BIN_EXE_ara"), &alternate).expect("alternate installed build");
    let output = Command::new(&alternate)
        .env("ARA_CONFIG_DIR", hub.config())
        .args(["hub", "register-engine", "--no-start"])
        .output()
        .expect("register alternate");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    hub.cli(&["hub", "unregister-engine", "--no-start"]);
    let registry = ara_hub_service::Registry::load(&hub.config()).expect("registry");
    assert_eq!(
        registry.engines.len(),
        1,
        "unregistering another build must not replace/remove this one"
    );
    assert_eq!(
        registry.engines[0].executable,
        ara_hub_service::normalized_path(
            &fs::canonicalize(&alternate).expect("alternate executable")
        )
    );
    assert!(ara_rpc::DaemonInfo::read(&hub.config().join("hub")).is_none());
    hub.cli(&["hub", "register-engine", "--no-start"]);
    hub.cli(&["hub", "projects"]);
    let pid = ara_rpc::DaemonInfo::read(&hub.config().join("hub"))
        .expect("explicit launch")
        .pid;
    hub.cli(&["hub", "register-engine", "--no-start"]);
    assert_eq!(
        ara_rpc::DaemonInfo::read(&hub.config().join("hub"))
            .expect("existing service")
            .pid,
        pid
    );
    let project = hub.create("Pinned");
    hub.cli(&[
        "hub",
        "open",
        project["id"].as_str().expect("id"),
        "--headless",
    ]);
    let refused = Command::new(env!("CARGO_BIN_EXE_ara"))
        .env("ARA_CONFIG_DIR", hub.config())
        .args(["hub", "unregister-engine", "--no-start"])
        .output()
        .expect("unregister open pin");
    assert!(
        !refused.status.success(),
        "open pins must still reject unregister"
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("close projects"));
    hub.cli(&["hub", "close", project["id"].as_str().expect("id")]);
    hub.cli(&["hub", "unregister-engine", "--no-start"]);
    assert!(
        hub.cli(&["hub", "engines"])["engines"]
            .as_array()
            .expect("engines")
            .is_empty()
    );
}

#[test]
fn cli_open_accepts_case_insensitive_names_and_reports_ambiguous_variants() {
    let hub = TestHub::new("case-insensitive-open");
    let first = hub.create("Starter");
    let opened = hub.cli(&["hub", "open", "starter", "--headless"]);
    assert_eq!(opened["project"]["id"], first["id"]);
    assert_eq!(opened["state"], "open");
    hub.cli(&["hub", "close", "STARTER"]);
    let second = hub.cli(&[
        "hub",
        "create",
        hub.directory.join("second").to_str().expect("path"),
        "--name",
        "STARTER",
    ]);
    let result = Command::new(env!("CARGO_BIN_EXE_ara"))
        .env("ARA_CONFIG_DIR", hub.config())
        .args(["hub", "open", "Starter", "--headless"])
        .output()
        .expect("ambiguous open");
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("ambiguous"), "{error}");
    for project in [first, second] {
        assert!(
            error.contains(project["id"].as_str().expect("id")),
            "{error}"
        );
    }
}

#[test]
fn service_build_handshake_replaces_idle_builds_and_preserves_open_projects() {
    let hub = TestHub::new("service-build");
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_ara"));
    let alternate = hub
        .directory
        .join(format!("preview9{}", std::env::consts::EXE_SUFFIX));
    fs::copy(&executable, &alternate).expect("copy service");
    // Executable overlays preserve execution, with a distinct build identity.
    fs::OpenOptions::new()
        .append(true)
        .open(&alternate)
        .expect("overlay")
        .write_all(b"different preview build")
        .expect("overlay bytes");
    let mut initial =
        ara_hub_service::connect_checked(&hub.config(), &executable).expect("initial service");
    let original = ara_rpc::DaemonInfo::read(&hub.config().join("hub"))
        .expect("discovery")
        .pid;
    let build = initial
        .client
        .call("hub.status", json!({}))
        .expect("status")["build"]
        .clone();
    assert_eq!(build.as_str().expect("build hash").len(), 64);
    let mut same = ara_hub_service::connect_checked(&hub.config(), &executable).expect("reuse");
    assert!(!same.update_pending);
    assert_eq!(
        ara_rpc::DaemonInfo::read(&hub.config().join("hub"))
            .expect("same pid")
            .pid,
        original
    );
    let project = same
        .client
        .call(
            "hub.create",
            json!({"name":"Live","template":"blank","path":hub.directory.join("Live")}),
        )
        .expect("create");
    let opened = same
        .client
        .call("hub.open", json!({"project":project["id"],"headless":true}))
        .expect("open");
    let mut busy =
        ara_hub_service::connect_checked(&hub.config(), &alternate).expect("keep busy service");
    assert!(busy.update_pending);
    assert_eq!(
        busy.client
            .call("hub.status", json!({"project":project["id"]}))
            .expect("still open")["pid"],
        opened["pid"]
    );
    assert_eq!(
        busy.client
            .call("hub.shutdown", json!({"onlyIfIdle":true}))
            .expect("conditional shutdown")["stopping"],
        false
    );
    busy.client
        .call("hub.close", json!({"project":project["id"]}))
        .expect("close");
    drop(busy);
    drop(same);
    drop(initial);
    let mut replacement =
        ara_hub_service::connect_checked(&hub.config(), &alternate).expect("replace idle build");
    assert!(!replacement.update_pending);
    assert_ne!(
        replacement
            .client
            .call("hub.status", json!({}))
            .expect("replacement status")["build"],
        build
    );
    assert_ne!(
        ara_rpc::DaemonInfo::read(&hub.config().join("hub"))
            .expect("new pid")
            .pid,
        original
    );
    assert!(
        !ara_hub_service::connect_checked(&hub.config(), &alternate)
            .expect("reuse replacement")
            .update_pending
    );
}

#[test]
fn legacy_service_without_build_is_gracefully_replaced() {
    use ara_proto::{ConnId, Incoming, jsonrpc};
    use ara_rpc::{DaemonInfo, Handler, ServeOptions};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Legacy(Arc<AtomicBool>);
    impl Handler for Legacy {
        fn message(&mut self, _: ConnId, text: &str) -> Option<String> {
            let Ok(Incoming::Request { id, method, params }) = Incoming::parse(text) else {
                return None;
            };
            let value = if method == "hub.shutdown" {
                assert_eq!(params, json!({}), "legacy previews cannot parse onlyIfIdle");
                self.0.store(true, Ordering::Release);
                json!({"stopping":true})
            } else {
                json!({"projects":[],"revision":0})
            };
            Some(jsonrpc::reply(id, Ok(value)).to_string())
        }
        fn stopping(&self) -> bool {
            self.0.load(Ordering::Acquire)
        }
    }
    let hub = TestHub::new("legacy-service");
    let discovery = hub.config().join("hub");
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("listener");
    let info = DaemonInfo {
        pid: std::process::id(),
        port: listener.local_addr().expect("port").port(),
        token: "legacy-test".into(),
        protocol: ara_proto::PROTOCOL.into(),
    };
    info.write(&discovery).expect("legacy discovery");
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let server = thread::spawn(move || {
        let result = ara_rpc::serve(
            &mut Legacy(flag),
            ServeOptions {
                listener: Some(listener),
                token: info.token,
                stdio: false,
                tick: Duration::from_millis(10),
            },
        );
        DaemonInfo::remove(&discovery);
        result.expect("legacy server");
    });
    let result =
        ara_hub_service::connect_checked(&hub.config(), Path::new(env!("CARGO_BIN_EXE_ara")));
    stop.store(true, Ordering::Release);
    server.join().expect("legacy stopped");
    let mut connected = result.expect("replace legacy");
    assert!(!connected.update_pending);
    assert!(
        connected
            .client
            .call("hub.status", json!({}))
            .expect("status")["build"]
            .is_string()
    );
}

struct Mcp {
    agent: &'static str,
    trace: fs::File,
    child: Child,
    input: ChildStdin,
    output: Receiver<String>,
    next: u64,
}
impl Mcp {
    fn start(hub: &TestHub, agent: &'static str) -> Self {
        let trace = fs::File::create(hub.directory.join(format!("mcp-{agent}.jsonl")))
            .expect("MCP transcript");
        let mut child = Command::new(env!("CARGO_BIN_EXE_ara"))
            .env("ARA_CONFIG_DIR", hub.config())
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("MCP");
        let input = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (send, output) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut mcp = Self {
            agent,
            trace,
            child,
            input,
            output,
            next: 0,
        };
        mcp.request(
            "initialize",
            json!({"protocolVersion":"2025-11-25","clientInfo":{"name":"hub-test","version":"1"}}),
        );
        mcp
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        let started = std::time::Instant::now();
        self.next += 1;
        let request = json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params});
        ara_proto::lines::write_message(&mut self.trace, &json!({"request":request}))
            .expect("record MCP request");
        writeln!(self.input, "{request}").expect("request");
        self.input.flush().expect("flush");
        let message = self
            .output
            .recv_timeout(Duration::from_secs(40))
            .unwrap_or_else(|error| {
                panic!(
                    "agent {} {method} #{} failed waiting for reply: {error}; request: {request}",
                    self.agent, self.next
                )
            });
        ara_proto::lines::write_message(
            &mut self.trace,
            &json!({"response":message,"elapsedMs":started.elapsed().as_millis()}),
        )
        .expect("record MCP reply");
        let message: Value = serde_json::from_str(&message).unwrap_or_else(|error| {
            panic!(
                "agent {} {method} #{} returned invalid JSON: {error}; response: {message}",
                self.agent, self.next
            )
        });
        assert!(
            message.get("error").is_none(),
            "agent {} {method} #{}; request: {request}; response: {message}",
            self.agent,
            self.next
        );
        message["result"].clone()
    }
    fn tool(&mut self, name: &str, args: Value) -> Value {
        let result = self.request("tools/call", json!({"name":name,"arguments":args}));
        assert_ne!(
            result["isError"], true,
            "agent {} {name}: {result}",
            self.agent
        );
        result["structuredContent"].clone()
    }
}
impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn create_entity(mcp: &mut Mcp, project: Option<&str>, name: &str) -> Value {
    let mut args = json!({"why":"Create a project-specific entity","commands":[{"cmd":"entity.create","as":"entity","args":{"name":name,"components":{"Transform":{}}}}]});
    if let Some(project) = project {
        args["project"] = project.into();
    }
    mcp.tool("ara_apply", args)
}

#[test]
fn global_mcp_edits_two_isolated_daemons_and_closes_one() {
    let hub = TestHub::new("two-projects");
    let a = hub.create("A");
    let b = hub.create("B");
    let mut mcp = Mcp::start(&hub, "projects");
    let list = mcp.tool("ara_projects", json!({"action":"list"}));
    assert_eq!(list["projects"].as_array().expect("projects").len(), 2);
    let tools = mcp.request("tools/list", json!({}));
    assert!(
        tools["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .all(|tool| tool["inputSchema"]["properties"].get("project").is_some())
    );
    let opened_a = mcp.tool(
        "ara_projects",
        json!({"action":"open","project":"A","headless":true}),
    );
    let session_a = mcp.tool(
        "ara_session",
        json!({"action":"open","project":a["id"],"client":"agent-a"}),
    );
    create_entity(&mut mcp, None, "Only A");
    let opened_b = mcp.tool(
        "ara_projects",
        json!({"action":"open","project":"B","headless":true}),
    );
    mcp.tool(
        "ara_session",
        json!({"action":"open","project":b["path"],"client":"agent-b"}),
    );
    create_entity(&mut mcp, None, "Only B");
    assert_ne!(opened_a["pid"], opened_b["pid"]);
    assert_ne!(opened_a["port"], opened_b["port"]);
    let query_a = mcp.tool(
        "ara_query",
        json!({"session":session_a["session"],"q":"name:Only*"}),
    );
    assert!(query_a.to_string().contains("Only A"));
    assert!(!query_a.to_string().contains("Only B"));
    let mismatch = mcp.request(
        "tools/call",
        json!({"name":"ara_query","arguments":{"project":"B","session":session_a["session"]}}),
    );
    assert_eq!(mismatch["isError"], true);
    let private = hub.directory.join("unregistered");
    fs::create_dir(&private).expect("private");
    let denied = mcp.request(
        "tools/call",
        json!({"name":"ara_query","arguments":{"project":private}}),
    );
    assert_eq!(denied["isError"], true);
    let closed = mcp.tool("ara_projects", json!({"action":"close","project":"A"}));
    assert_eq!(closed["state"], "closed");
    assert_eq!(
        mcp.tool("ara_projects", json!({"action":"status","project":"B"}))["state"],
        "open"
    );
    let scene: Value = serde_json::from_slice(
        &fs::read(hub.directory.join("A/levels/main.scene")).expect("saved A"),
    )
    .expect("scene");
    assert!(scene.to_string().contains("Only A"));
    assert!(!scene.to_string().contains("Only B"));
}

#[test]
fn concurrent_global_agents_keep_project_sessions_isolated() {
    let hub = TestHub::new("concurrent");
    hub.create("A");
    hub.create("B");
    let mut a = Mcp::start(&hub, "A");
    let mut b = Mcp::start(&hub, "B");
    let run = |mut mcp: Mcp, project: &'static str| {
        thread::spawn(move || {
            mcp.tool(
                "ara_session",
                json!({"action":"open","project":project,"client":project}),
            );
            for i in 0..3 {
                create_entity(&mut mcp, None, &format!("{project} {i}"));
            }
            let entities = mcp.tool("ara_query", json!({"q":format!("name:{project}*")}));
            assert!(entities.to_string().contains(&format!("{project} 2")));
        })
    };
    // First explicit calls deliberately start their daemons concurrently.
    a.tool("ara_projects", json!({"action":"list"}));
    b.tool("ara_projects", json!({"action":"list"}));
    let first = run(a, "A");
    let second = run(b, "B");
    first.join().expect("agent A");
    second.join().expect("agent B");
    let status = hub.client().call("hub.status", json!({})).expect("status");
    let projects = status["projects"].as_array().expect("projects");
    assert!(projects.iter().all(|s| s["state"] == "open"));
    assert_ne!(projects[0]["pid"], projects[1]["pid"]);
}

#[test]
fn registry_metadata_subscriptions_templates_and_global_install_configs() {
    let hub = TestHub::new("registry");
    let a = hub.create("A");
    let mut client = hub.client();
    client.call("hub.subscribe", json!({})).expect("subscribe");
    client.call("hub.update",json!({"project":a["id"],"name":"Renamed","thumbnail":hub.directory.join("preview.png")})).expect("update");
    assert!(
        client
            .wait_notification(Duration::from_secs(2))
            .is_some_and(|n| n["method"] == "hub.changed")
    );
    let projects = client.call("hub.projects", json!({})).expect("list");
    assert_eq!(projects["projects"][0]["name"], "Renamed");
    let marker: Value = serde_json::from_slice(
        &fs::read(hub.directory.join("A/ara.project.json")).expect("marker"),
    )
    .expect("marker JSON");
    assert_eq!(marker["engineVersion"], ara_hub_service::ALPHA_VERSION);
    assert_eq!(
        client.call("hub.templates", json!({})).expect("templates")["templates"][0]["id"],
        "blank"
    );
    client
        .call("hub.remove", json!({"project":"Renamed"}))
        .expect("remove");
    assert!(hub.directory.join("A/ara.project.json").is_file());
    let config = hub.cli(&["hub", "install-mcp", "--client", "claude"]);
    assert_eq!(
        config["mcpServers"]["ara"]["command"],
        json!(cli_executable())
    );
    assert_eq!(config["mcpServers"]["ara"]["args"], json!(["mcp"]));
    let codex = Command::new(env!("CARGO_BIN_EXE_ara"))
        .env("ARA_CONFIG_DIR", hub.config())
        .args(["hub", "install-mcp", "--client", "codex"])
        .output()
        .expect("config");
    assert!(String::from_utf8_lossy(&codex.stdout).contains("[mcp_servers.ara]"));
    let quoted = serde_json::to_string(&cli_executable()).expect("quoted path");
    assert!(String::from_utf8_lossy(&codex.stdout).contains(&format!("command = {quoted}")));
}
