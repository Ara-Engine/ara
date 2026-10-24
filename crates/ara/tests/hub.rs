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

struct Mcp {
    child: Child,
    input: ChildStdin,
    output: Receiver<String>,
    next: u64,
}
impl Mcp {
    fn start(hub: &TestHub) -> Self {
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
        self.next += 1;
        writeln!(
            self.input,
            "{}",
            json!({"jsonrpc":"2.0","id":self.next,"method":method,"params":params})
        )
        .expect("request");
        self.input.flush().expect("flush");
        let message = self
            .output
            .recv_timeout(Duration::from_secs(40))
            .expect("MCP response timeout");
        let message: Value = serde_json::from_str(&message).expect("response JSON");
        assert!(message.get("error").is_none(), "{message}");
        message["result"].clone()
    }
    fn tool(&mut self, name: &str, args: Value) -> Value {
        let result = self.request("tools/call", json!({"name":name,"arguments":args}));
        assert_ne!(result["isError"], true, "{name}: {result}");
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
    let mut mcp = Mcp::start(&hub);
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
    let mut a = Mcp::start(&hub);
    let mut b = Mcp::start(&hub);
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
    assert_eq!(config["mcpServers"]["ara"]["args"], json!(["mcp"]));
    let codex = Command::new(env!("CARGO_BIN_EXE_ara"))
        .env("ARA_CONFIG_DIR", hub.config())
        .args(["hub", "install-mcp", "--client", "codex"])
        .output()
        .expect("config");
    assert!(String::from_utf8_lossy(&codex.stdout).contains("[mcp_servers.ara]"));
}
