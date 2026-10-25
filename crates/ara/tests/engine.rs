//! `ara init`, root discovery, protocol commands (offline and through the daemon),
//! `ara docs --llms`, and the end-to-end MCP flow against `ara serve`.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-engine-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Runs `ara <args>` in `cwd`; returns (exit code, stdout, stderr).
fn ara_in(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    ara_env(cwd, args, &[])
}

/// [`ara_in`] with extra environment; agent markers of the shell running the tests
/// are cleared so the default author is `human`.
fn ara_env(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ara"));
    for key in ["CLAUDECODE", "CODEX_THREAD_ID", "ARA_AUTHOR"] {
        cmd.env_remove(key);
    }
    let out = cmd
        .current_dir(cwd)
        .args(args)
        .envs(env.iter().copied())
        .output()
        .expect("ara runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

#[test]
fn authoring_creates_documents_sets_main_and_configures_sidecars_across_runs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/authoring-tests")
        .join(format!("cli-documents-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch project");
    assert_eq!(ara_in(&root, &["init"]).0, 0);
    let apply = |command: &str, args: &str| {
        let (code, output, error) = ara_in(
            &root,
            &[
                "do",
                command,
                "--args",
                args,
                "--why",
                "Test shared authoring",
                "--json",
            ],
        );
        assert_eq!(code, 0, "{output}{error}");
        json_of(&output)
    };
    apply(
        "doc.create",
        r#"{"path":"levels/second.scene","id":"sc_second"}"#,
    );
    apply("project.setMain", r#"{"scene":"@levels/second.scene"}"#);
    let marker =
        json_of(&fs::read_to_string(root.join("ara.project.json")).expect("project marker"));
    assert_eq!(marker["main"], "levels/second.scene");
    fs::create_dir_all(root.join("assets")).expect("assets directory");
    fs::write(root.join("assets/model.gltf"), "{}")
        .expect("source bytes; cooking is a separate operation");
    let created = apply(
        "import.initialize",
        r#"{"source":"assets/model.gltf","id":"as_model"}"#,
    );
    assert_eq!(created["outputs"][0]["importer"], "ara.gltf");
    assert!(
        created["outputs"][0]["version"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
    );
    apply(
        "import.configure",
        r#"{"source":"assets/model.gltf","settings":{"lodCount":2},"preset":"fast"}"#,
    );
    let settings =
        json_of(&fs::read_to_string(root.join("assets/model.gltf.import.json")).expect("sidecar"));
    assert_eq!(settings["records"]["i_settings"]["settings"]["lodCount"], 2);
    assert_eq!(
        settings["records"]["i_settings"]["version"],
        created["outputs"][0]["version"]
    );
    let (code, output, error) = ara_in(&root, &["undo", "--json"]);
    assert_eq!(code, 0, "{output}{error}");
    let settings = json_of(
        &fs::read_to_string(root.join("assets/model.gltf.import.json")).expect("restored sidecar"),
    );
    assert!(
        settings["records"]["i_settings"]["settings"]
            .get("lodCount")
            .is_none()
    );
    let (code, output, error) = ara_in(&root, &["undo", "--json"]);
    assert_eq!(code, 0, "{output}{error}");
    assert!(!root.join("assets/model.gltf.import.json").exists());
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    for entry in fs::read_dir(from).expect("read dir").flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

#[test]
fn commands_need_a_project_marker_and_skip_nested_projects() {
    let bare = scratch("bare");
    let (code, out, _) = ara_in(&bare, &["check", "--json"]);
    assert_eq!(code, 2);
    let d = &json_of(&out)["diagnostics"][0];
    assert_eq!(d["code"], "ARA1901");
    assert_eq!(d["fixes"][0]["action"], "cli");
    assert_eq!(d["fixes"][0]["args"][0], "init");

    // A project whose subdirectories hold a nested project, spikes and hidden dirs:
    // none of their (broken) documents belong to it.
    let root = scratch("nested");
    let (code, _, err) = ara_in(&root, &["init", "--name", "outer"]);
    assert_eq!(code, 0, "{err}");
    for dir in ["inner", "spikes/s9", ".refs/godot", "target/x"] {
        fs::create_dir_all(root.join(dir)).expect("mkdir");
        fs::write(root.join(dir).join("bad.scene"), "{ not json").expect("write");
    }
    fs::write(root.join("inner/ara.project.json"), "{}").expect("marker");
    let (code, out, _) = ara_in(&root.join("levels"), &["check", "--json"]);
    let report = json_of(&out);
    assert_eq!(
        (code, report["files"].clone(), report["errors"].clone()),
        (0, json!(2), json!(0)),
        "{out}"
    );
}

#[test]
fn init_writes_the_project_template() {
    let root = scratch("init");
    let (code, out, _) = ara_in(&root, &["init", "--json"]);
    assert_eq!(code, 0);
    let report = json_of(&out);
    let created: Vec<&str> = report["created"]
        .as_array()
        .expect("created")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for f in [
        "ara.project.json",
        ara_asset::PROJECT_IMPORT_SETTINGS_FILE,
        "AGENTS.md",
        ".gitattributes",
        ".gitignore",
        ".mcp.json",
        "levels/main.scene",
    ] {
        assert!(created.contains(&f), "{f} not created: {created:?}");
    }
    let agents = fs::read_to_string(root.join("AGENTS.md")).expect("AGENTS.md");
    for convention in [
        "Y-up",
        "meters",
        "degrees",
        "@path#id",
        "Use commands, not raw file surgery",
    ] {
        assert!(
            agents.contains(convention),
            "AGENTS.md lacks `{convention}`"
        );
    }
    assert!(
        fs::read_to_string(root.join(".gitattributes"))
            .expect("attrs")
            .contains("*.scene merge=ara")
    );
    assert!(
        fs::read_to_string(root.join(".gitignore"))
            .expect("ignore")
            .contains(".ara/")
    );
    let mcp = json_of(&fs::read_to_string(root.join(".mcp.json")).expect("mcp"));
    assert_eq!(
        mcp["mcpServers"]["ara"]["args"],
        json!(["mcp", "--root", "."])
    );

    let (code, _, _) = ara_in(&root, &["check"]);
    assert_eq!(code, 0, "a fresh project checks clean");
    let (code, out, _) = ara_in(&root, &["init", "--json"]);
    assert_eq!(
        (code, json_of(&out)["diagnostics"][0]["code"].clone()),
        (1, json!("ARA1902"))
    );
}

#[test]
fn protocol_commands_work_offline_and_keep_history_between_runs() {
    let root = scratch("offline");
    ara_in(&root, &["init"]);
    let scene = root.join("levels/main.scene");
    let empty = fs::read_to_string(&scene).expect("scene");

    let (code, out, err) = ara_in(
        &root,
        &[
            "do",
            "entity.create",
            "--doc",
            "levels/main.scene",
            "-p",
            "name=Crate",
            "-p",
            "components.Transform.position=[1,2,3]",
            "--as",
            "$c",
            "--why",
            "a crate",
            "--author",
            "agent:claude",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{out}{err}");
    let applied = json_of(&out);
    let id = applied["ids"]["$c"]
        .as_str()
        .expect("symbol mapped")
        .to_owned();
    assert!(
        fs::read_to_string(&scene).expect("scene").contains(&id),
        "flushed on exit"
    );

    let (_, out, _) = ara_in(
        &root,
        &["query", "name:Crate", "--fields", "Transform.position"],
    );
    assert!(out.contains(&format!("{id} Crate pos(1,2,3)")), "{out}");
    let (_, out, _) = ara_in(&root, &["review", "--json"]);
    assert_eq!(json_of(&out)["items"], json!([]));
    let (code, out, _) = ara_in(
        &root,
        &["accept", "--all", "--author", "agent:claude", "--json"],
    );
    assert_eq!(code, 0, "{out}");
    let (code, out, _) = ara_in(&root, &["revert", "--entity", &id]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(
        fs::read_to_string(&scene).expect("scene"),
        empty,
        "reverted on disk"
    );

    let (code, out, _) = ara_in(&root, &["undo", "--author", "agent:claude", "--json"]);
    assert_eq!(code, 1, "the revert already undid it: {out}");
    let (_, out, _) = ara_in(&root, &["history", "--json"]);
    assert_eq!(
        json_of(&out)["items"].as_array().map(Vec::len),
        Some(2),
        "history survives runs"
    );
    let (code, out, _) = ara_in(
        &root,
        &["do", "entity.creat", "--doc", "levels/main.scene", "--json"],
    );
    assert_eq!(code, 1);
    let d = &json_of(&out)["diagnostics"][0];
    assert_eq!(d["code"], "ARA1601");
    assert!(
        d["message"]
            .as_str()
            .is_some_and(|m| m.contains("`entity.create`"))
    );
}

#[test]
fn docs_llms_writes_versioned_pages() {
    let root = scratch("docs");
    ara_in(&root, &["init"]);
    let (code, out, _) = ara_in(&root, &["docs", "--llms", "--json"]);
    assert_eq!(code, 0, "{out}");
    let docs = root.join(".ara/docs");
    let llms = fs::read_to_string(docs.join("llms.txt")).expect("llms.txt");
    assert!(llms.starts_with("# Ara · "));
    assert!(llms.contains(&format!(
        "Generated by ara {} · protocol ara-ep/1",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(llms.contains("[Command `entity.create`](commands/entity.create.md)"));
    let transform = fs::read_to_string(docs.join("types/Transform.md")).expect("type page");
    assert!(transform.contains("| `rotation` | `vec3` |") && transform.contains("deg"));
    assert!(docs.join("methods/cmd.apply.md").exists());
    assert!(
        fs::read_to_string(docs.join("llms-full.txt"))
            .expect("full")
            .contains("# Diagnostic codes")
    );
}

/// A child process killed on drop.
struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// An `ara mcp` child speaking JSON lines.
struct Mcp {
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next: u64,
    proc: Proc,
}

impl Mcp {
    fn start(root: &Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ara"))
            .current_dir(root)
            .arg("mcp")
            .arg("--root")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("ara mcp starts");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Mcp {
            stdin: Some(stdin),
            lines,
            next: 1,
            proc: Proc(child),
        }
    }

    /// Closes stdin (the MCP client went away) and waits for a clean exit.
    fn close(mut self) {
        self.stdin = None;
        wait_for("ara mcp to exit", || {
            matches!(self.proc.0.try_wait(), Ok(Some(_)))
        });
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let mut params = params;
        params["_meta"] = json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28" });
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let stdin = self.stdin.as_mut().expect("open");
        writeln!(stdin, "{msg}").expect("send");
        stdin.flush().expect("flush");
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| panic!("no answer to {method}"));
        let resp = json_of(&line);
        assert_eq!(resp["id"], id, "{line}");
        resp
    }

    fn tool(&mut self, name: &str, args: Value) -> Value {
        let resp = self.request("tools/call", json!({ "name": name, "arguments": args }));
        let result = resp["result"].clone();
        assert_eq!(
            result["isError"], false,
            "{name} failed: {}",
            result["content"][0]["text"]
        );
        result["structuredContent"].clone()
    }
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !ok() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_mcp_agent_edits_through_the_daemon_and_reverts_its_turn() {
    let root = scratch("e2e");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project"),
        &root,
    );
    let scene = root.join("levels/arena.scene");
    let original = fs::read_to_string(&scene).expect("scene");

    let daemon = Proc(
        Command::new(env!("CARGO_BIN_EXE_ara"))
            .current_dir(&root)
            .arg("serve")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("ara serve starts"),
    );
    wait_for("the daemon file", || root.join(".ara/daemon.json").exists());

    let mut mcp = Mcp::start(&root);
    let tools = mcp.request("tools/list", json!({}));
    assert_eq!(tools["result"]["tools"].as_array().map(Vec::len), Some(13));

    let begun = mcp.tool(
        "ara_session",
        json!({ "action": "begin", "client": "claude-code" }),
    );
    let session = begun["session"].as_str().expect("session").to_owned();
    assert_eq!(begun["author"], "agent:claude-code");

    let applied = mcp.tool(
        "ara_apply",
        json!({
            "session": session, "doc": "levels/arena.scene", "why": "Add a beacon above the floor",
            "commands": [
                { "cmd": "entity.create", "as": "$beacon", "args": { "name": "Beacon", "parent": "e_flr1", "components": { "Transform": { "position": [0, 3, 0] } } } },
                { "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Deck" } }
            ]
        }),
    );
    let beacon = applied["ids"]["$beacon"]
        .as_str()
        .expect("symbol")
        .to_owned();
    let txn = applied["txn"].as_str().expect("txn").to_owned();
    wait_for("the edit on disk", || {
        fs::read_to_string(&scene).is_ok_and(|t| t.contains(&beacon) && t.contains("\"Deck\""))
    });

    let found = mcp.tool(
        "ara_query",
        json!({ "session": session, "q": "name:Beacon", "doc": "levels/arena.scene" }),
    );
    assert_eq!(found["items"][0]["entity"], beacon.as_str());
    assert!(
        found["text"]
            .as_str()
            .is_some_and(|t| t.contains("pos(0,3,0)"))
    );

    // The CLI talks to the same daemon and sees the agent's turn.
    let (code, out, _) = ara_in(&root, &["history", "--json", "--limit", "1"]);
    assert_eq!(code, 0, "{out}");
    let latest = &json_of(&out)["items"][0];
    assert_eq!(
        (latest["txn"].as_str(), latest["status"].as_str()),
        (Some(txn.as_str()), Some("accepted"))
    );
    assert_eq!(latest["why"], "Add a beacon above the floor");

    let reverted = mcp.tool(
        "ara_session",
        json!({ "action": "revert", "session": session, "txn": txn }),
    );
    assert_eq!(reverted["reverted"][0]["txn"], txn.as_str());
    wait_for("the revert on disk", || {
        fs::read_to_string(&scene).is_ok_and(|t| t == original)
    });
    drop(mcp);
    drop(daemon);
}

/// POSTs one MCP message to the daemon's `/mcp` endpoint; returns (status, body).
fn mcp_http(port: u16, token: &str, headers: &str, body: &Value) -> (u16, String) {
    use std::io::Read;
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let text = body.to_string();
    write!(
        s,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{headers}Content-Length: {}\r\n\r\n{text}",
        text.len()
    )
    .expect("write");
    let mut out = String::new();
    s.read_to_string(&mut out).expect("read");
    let status = out
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (
        status,
        out.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned(),
    )
}

#[test]
fn mcp_without_a_daemon_becomes_one_and_serves_http_too() {
    let root = scratch("mcp-daemon");
    ara_in(&root, &["init"]);
    let mut mcp = Mcp::start(&root);
    let begun = mcp.tool(
        "ara_session",
        json!({ "action": "begin", "client": "codex" }),
    );
    let session = begun["session"].as_str().expect("session").to_owned();
    mcp.tool(
        "ara_apply",
        json!({ "session": session, "doc": "levels/main.scene", "why": "first entity",
                "commands": [{ "cmd": "entity.create", "args": { "name": "Probe" } }] }),
    );

    let info = json_of(
        &fs::read_to_string(root.join(".ara/daemon.json")).expect("ara mcp wrote the daemon file"),
    );
    let (port, token) = (
        info["port"].as_u64().expect("port") as u16,
        info["token"].as_str().expect("token").to_owned(),
    );
    let (code, out, _) = ara_in(&root, &["history", "--json"]);
    assert_eq!(code, 0, "the CLI uses the running engine: {out}");
    assert_eq!(json_of(&out)["items"][0]["why"], "first entity");

    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                       "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } } });
    let (status, body) = mcp_http(
        port,
        &token,
        "MCP-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/list\r\n",
        &list,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        json_of(&body)["result"]["tools"].as_array().map(Vec::len),
        Some(13)
    );
    let (status, _) = mcp_http(port, "wrong-token", "", &list);
    assert_eq!(status, 401);

    mcp.close();
    assert!(
        !root.join(".ara/daemon.json").exists(),
        "removed on a clean exit"
    );
}

#[test]
fn native_ui_and_material_documents_are_created_and_formatted_through_the_cli() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/authoring-tests")
        .join(format!("cli-native-docs-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch project");
    let (code, out, err) = ara_in(&root, &["init"]);
    assert_eq!(code, 0, "{out}{err}");
    for args in [
        json!({"path":"levels/second.scene","id":"sc_second"}),
        json!({"path":"prefabs/empty.prefab","id":"pf_empty"}),
        json!({"path":"terrain/empty.terrain","id":"tr_empty"}),
        json!({"path":"terrain/empty.foliage","id":"fo_empty"}),
        json!({"path":"anim/empty.animation","id":"ac_empty"}),
        json!({"path":"audio/empty.music","id":"mu_empty"}),
        json!({"path":"ui/hud.ui","id":"ui_hud"}),
        json!({"path":"materials/plain.graph","kind":"material","id":"gr_plain"}),
    ] {
        let args = args.to_string();
        let (code, out, err) = ara_in(
            &root,
            &[
                "do",
                "doc.create",
                "--args",
                &args,
                "--why",
                "Create a native asset",
            ],
        );
        assert_eq!(code, 0, "{out}{err}");
    }
    let source = fs::read_to_string(root.join("ui/hud.ui")).expect("UI source");
    assert!(source.starts_with("<!-- ara:asset ui_hud -->"));
    assert!(source.contains("<ui id=\"ui_hud\" />"));
    let (code, out, err) = ara_in(
        &root,
        &["fmt", "--check", "ui/hud.ui", "materials/plain.graph"],
    );
    assert_eq!(code, 0, "{out}{err}");
    let (code, out, err) = ara_in(&root, &["check"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(
        fs::read_to_string(root.join("ui/hud.ui")).expect("source unchanged"),
        source
    );
}

#[test]
fn agent_shells_are_attributed_and_args_survive_stripped_quotes() {
    let root = scratch("agent-cli");
    ara_in(&root, &["init"]);
    let codex = [("CODEX_THREAD_ID", "thread-1")];

    // No `--doc` (the only scene), no `--author` (Codex's shell), no `--why`.
    let (code, out, _) = ara_env(
        &root,
        &["do", "entity.create", "-p", "name=Crate", "--json"],
        &codex,
    );
    assert_eq!(code, 1, "{out}");
    assert_eq!(json_of(&out)["diagnostics"][0]["code"], "ARA1610");

    // JSON as PowerShell 5.1 delivers it: quotes stripped.
    let (code, out, err) = ara_env(
        &root,
        &[
            "do",
            "entity.create",
            "--args",
            "{name: Crate, components: {Transform: {position: [1, 2, 3]}}}",
            "--why",
            "a crate",
        ],
        &codex,
    );
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.starts_with("applied tx_") && out.contains(" Crate"),
        "{out}"
    );
    let (code, out, err) = ara_env(
        &root,
        &[
            "do",
            "transform.set",
            "-p",
            "entities=[name:Crate]",
            "-p",
            "position=[0,1,0]",
            "--why",
            "lift it",
        ],
        &codex,
    );
    assert_eq!(code, 0, "{out}{err}");
    let (_, out, _) = ara_in(&root, &["review", "--json"]);
    assert_eq!(json_of(&out)["items"], json!([]));
    let (_, out, _) = ara_in(&root, &["history", "--json"]);
    assert_eq!(json_of(&out)["items"][0]["author"], "agent:codex");

    // A whole batch without a command name; `as` may skip its `$`.
    let batch = "{why: two lamps, commands: [{command: entity.create, args: {name: Lamp}, as: a}, {cmd: entity.create, args: {name: Lamp2, parent: $a}}]}";
    let (code, out, err) = ara_env(&root, &["do", "--args", batch], &codex);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("Lamp ($a)") && out.contains("Lamp2"), "{out}");
    let (_, out, _) = ara_in(&root, &["inspect", "Crate"]);
    assert!(out.contains("Crate pos(0,1,0)"), "{out}");
}
