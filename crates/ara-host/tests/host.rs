//! The host through its protocol surface: round trips, scopes, dry runs, symbols,
//! review, rollback, crash recovery, external edits and service registration.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ara_diag::{Diagnostic, FixAction, codes};
use ara_host::{Cx, Host, HostConfig, Method, Project, Service};
use ara_proto::{ConnId, Scope};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    for entry in fs::read_dir(from).expect("read fixture dir").flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

/// A fresh copy of the fixture project for one test.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ara-host-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&fixtures(), &dir);
    dir
}

/// A host that never flushes on its own (only on `flush`, drop or `doc.sync`) and
/// checks files on every tick.
fn open(dir: &Path, seed: u64) -> Host {
    let config = HostConfig {
        debounce: Duration::from_secs(3600),
        watch_every: Duration::ZERO,
        scan_every: Duration::ZERO,
        seed: Some(seed),
        ..HostConfig::default()
    };
    Host::open(Project::open(dir).expect("project"), config).expect("host")
}

fn call(h: &mut Host, method: &str, params: Value) -> Value {
    h.call(ConnId::LOCAL, method, params)
        .unwrap_or_else(|e| panic!("{method} failed:\n{e}"))
}

fn fail(h: &mut Host, method: &str, params: Value) -> Diagnostic {
    h.call(ConnId::LOCAL, method, params)
        .expect_err(method)
        .diagnostics()
        .remove(0)
}

fn begin(h: &mut Host, author: &str) -> String {
    let r = call(
        h,
        "session.begin",
        json!({ "client": "test", "author": author }),
    );
    r["session"].as_str().expect("handle").to_owned()
}

#[test]
fn source_replace_has_offline_undo_redo_and_rejects_external_changes() {
    fn complete(h: &mut Host, method: &str, params: Value) -> Result<Value, Vec<Diagnostic>> {
        let session = params["session"].clone();
        let result = h
            .call(ConnId::LOCAL, method, params)
            .map_err(|e| e.diagnostics())?;
        let Some(job) = result["job"].as_str() else {
            return Ok(result);
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            h.tick();
            let status = call(h, "jobs.status", json!({"session":session,"job":job}));
            if status["status"] == "done" {
                return Ok(status["result"].clone());
            }
            if status["status"] == "failed" {
                return Err(
                    serde_json::from_value(status["diagnostics"].clone()).expect("diagnostics")
                );
            }
            assert!(
                std::time::Instant::now() < deadline,
                "source job timed out: {status}"
            );
            std::thread::yield_now();
        }
    }
    let dir = std::env::var_os("ARA_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/authoring-tests")
        })
        .join(format!("source-replace-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&fixtures(), &dir);
    fs::create_dir_all(dir.join("content/scripts/Game")).expect("module directory");
    let path = "content/scripts/Game/Door.cs";
    fs::write(dir.join(path), "before\n").expect("source");
    let mut h = open(&dir, 2041);
    let session = begin(&mut h, "agent:fixer");
    let base = "9160d4be34c8695bd172a76c7c7966587ea5a4d991ad22c87b2b91af54aa9ebb";
    let request = json!({"session":session,"why":"Fix the script","commands":[
        {"cmd":"source.replace","args":{"path":path,"baseHash":base,"text":"after\n"}}
    ]});
    let result = complete(&mut h, "cmd.apply", request).expect("source applied");
    assert!(result["txn"].is_string(), "{result}");
    h.flush();
    assert_eq!(
        fs::read_to_string(dir.join(path)).expect("replaced"),
        "after\n"
    );
    drop(h);
    // A crash after the journal entry but before the byte write leaves this state.
    let journal = dir.join(".ara/journal");
    let interrupted = fs::read_to_string(&journal)
        .expect("journal")
        .lines()
        .filter(|line| !line.contains("\"t\":\"fileFlush\""))
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    fs::write(journal, interrupted).expect("simulate crash before file flush acknowledgment");
    fs::write(dir.join(path), "before\n").expect("simulate interrupted materialization");
    let mut h = open(&dir, 2042);
    assert_eq!(
        fs::read_to_string(dir.join(path)).expect("recovered bytes"),
        "after\n"
    );
    let session = begin(&mut h, "agent:fixer");
    complete(&mut h, "history.undo", json!({"session":session})).expect("undo");
    h.flush();
    assert_eq!(
        fs::read_to_string(dir.join(path)).expect("undone"),
        "before\n"
    );
    drop(h);
    let mut h = open(&dir, 2043);
    let session = begin(&mut h, "agent:fixer");
    complete(&mut h, "history.redo", json!({"session":session})).expect("redo");
    h.flush();
    assert_eq!(
        fs::read_to_string(dir.join(path)).expect("redone"),
        "after\n"
    );
    fs::write(dir.join(path), "external\n").expect("external editor");
    let errors =
        complete(&mut h, "history.undo", json!({"session":session})).expect_err("stale file");
    assert_eq!(errors[0].code, codes::STALE_BASE);
    assert_eq!(
        fs::read_to_string(dir.join(path)).expect("preserved"),
        "external\n"
    );
    fs::write(dir.join(path), "before\n").expect("external editor restores an earlier revision");
    drop(h);
    let mut reopened = open(&dir, 2044);
    assert_eq!(
        fs::read_to_string(dir.join(path)).expect("external known bytes"),
        "before\n",
        "a completed flush must never be replayed over an intentional external restoration"
    );
    let session = begin(&mut reopened, "agent:fixer");
    let errors = complete(&mut reopened, "history.undo", json!({"session":session}))
        .expect_err("external restoration is a stale undo base");
    assert_eq!(errors[0].code, codes::STALE_BASE);
}

#[test]
fn native_ui_creation_and_source_ops_preserve_xml_across_restart_and_undo() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/authoring-tests")
        .join(format!("native-ui-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&fixtures(), &dir);
    let mut h = open(&dir, 2040);
    let session = begin(&mut h, "agent:builder");
    let request = |dry| {
        json!({"session":session,"why":"Create the HUD","dryRun":dry,"commands":[
            {"cmd":"doc.create","args":{"path":"ui/hud.ui","id":"ui_hud"}}
        ]})
    };
    let preview = call(&mut h, "cmd.apply", request(true));
    let applied = call(&mut h, "cmd.apply", request(false));
    assert_eq!(preview["ops"], applied["ops"]);
    assert!(!dir.join("content/ui/hud.ui").exists());
    call(
        &mut h,
        "cmd.apply",
        json!({"session":session,"doc":"ui/hud.ui","why":"Add the title","commands":[
            {"cmd":"ui.source.apply","args":{"source":"<?xml version=\"1.0\"?>\n<ui id=\"hud\"><!-- keep --><text id=\"title\" text=\"Hello &amp; welcome\" /></ui>\n"}}
        ]}),
    );
    h.flush();
    let text = fs::read_to_string(dir.join("content/ui/hud.ui")).expect("native source");
    assert!(text.starts_with("<?xml"));
    assert!(text.contains("<!-- ara:asset ui_hud -->"));
    assert!(text.contains("<!-- keep -->"));
    assert!(text.contains("Hello &amp; welcome"));
    let project = Project::open(&dir).expect("project");
    let loaded = project.read_document(&text, "ui/hud.ui");
    assert!(loaded.diags.is_empty(), "{:?}", loaded.diags);
    assert_eq!(
        project
            .write_document(&loaded.doc.expect("UI document"))
            .expect("writer"),
        text
    );
    h.kill();
    let mut h = open(&dir, 2041);
    let session = begin(&mut h, "agent:builder");
    call(&mut h, "history.undo", json!({"session":session}));
    h.flush();
    let text = fs::read_to_string(dir.join("content/ui/hud.ui")).expect("empty native source");
    assert!(text.contains("<ui id=\"ui_hud\" />"));
    call(&mut h, "history.undo", json!({"session":session}));
    h.flush();
    assert!(!dir.join("content/ui/hud.ui").exists());
}

#[test]
#[allow(clippy::unwrap_used)]
fn recovery_replays_only_unsaved_documents_after_a_partial_flush() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/authoring-tests")
        .join(format!("partial-flush-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&fixtures(), &dir);
    let mut h = open(&dir, 2030);
    let session = begin(&mut h, "agent:builder");
    let created = call(
        &mut h,
        "cmd.apply",
        json!({"session":session,"why":"Create two levels","commands":[
            {"cmd":"doc.create","args":{"path":"levels/first.scene","id":"sc_first"}},
            {"cmd":"doc.create","args":{"path":"levels/second.scene","id":"sc_second"}}
        ]}),
    );
    h.flush();
    h.kill();
    // Simulate a crash after the first file's rename and flush journal entry.
    fs::remove_file(dir.join("content/levels/second.scene")).unwrap();
    let journal = dir.join(".ara/journal");
    let text = fs::read_to_string(&journal).unwrap();
    let text = text
        .lines()
        .filter(|line| {
            let entry: Value = serde_json::from_str(line).unwrap();
            !(entry["t"] == "flush" && entry["doc"] == "sc_second")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(journal, text).unwrap();
    let mut h = open(&dir, 2031);
    let docs = call(&mut h, "session.begin", json!({"client":"recovery"}));
    let docs = docs["documents"].as_array().unwrap();
    assert!(docs.iter().any(|d| d["id"] == "sc_first"));
    assert!(docs.iter().any(|d| d["id"] == "sc_second"));
    h.flush();
    assert!(dir.join("content/levels/second.scene").is_file());
    let session = begin(&mut h, "agent:builder");
    call(
        &mut h,
        "history.undo",
        json!({"session":session,"txn":created["txn"]}),
    );
    h.flush();
    assert!(!dir.join("content/levels/first.scene").exists());
    assert!(!dir.join("content/levels/second.scene").exists());
}

#[test]
fn document_creation_and_main_scene_survive_crash_and_undo() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/authoring-tests")
        .join(format!("host-documents-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&fixtures(), &dir);
    let mut h = open(&dir, 2026);
    let session = begin(&mut h, "agent:builder");
    let request = |dry| json!({"session":session,"why":"Create a second level","dryRun":dry,"commands":[{"cmd":"doc.create","as":"level","args":{"path":"levels/new.scene"}}]});
    let dry = call(&mut h, "cmd.apply", request(true));
    assert!(!dir.join("content/levels/new.scene").exists());
    let created = call(&mut h, "cmd.apply", request(false));
    assert_eq!(dry["ops"], created["ops"]);
    assert_eq!(dry["ids"], created["ids"]);
    assert!(created["txn"].is_string());
    call(
        &mut h,
        "cmd.apply",
        json!({"session":session,"why":"Start in the new level","commands":[{"cmd":"project.setMain","args":{"scene":"@levels/new.scene"}}]}),
    );
    h.kill();
    let mut h = open(&dir, 2027);
    let resumed = begin(&mut h, "agent:builder");
    assert!(
        dir.join("content/levels/new.scene").is_file(),
        "unflushed document recovered"
    );
    let marker: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("ara.project.json")).expect("marker"))
            .expect("settings");
    assert_eq!(marker["main"], "levels/new.scene");
    call(&mut h, "history.undo", json!({"session":resumed}));
    call(&mut h, "history.undo", json!({"session":resumed}));
    h.flush();
    assert!(
        !dir.join("content/levels/new.scene").exists(),
        "undo removes the empty skeleton"
    );
    drop(h);
    let mut h = open(&dir, 2028);
    let resumed = begin(&mut h, "agent:builder");
    assert!(
        !dir.join("content/levels/new.scene").exists(),
        "recovery respects the deletion flush"
    );
    // A source file must never be overwritten by a document command.
    let d = fail(
        &mut h,
        "cmd.apply",
        json!({"session":resumed,"why":"Collision test","commands":[{"cmd":"doc.create","args":{"path":"levels/arena.scene"}}]}),
    );
    assert_eq!(d.code, codes::INVALID_PARAMS);
}

#[test]
fn explicit_document_deletion_flushes_recovers_and_undoes_with_history() {
    let dir = project("document-delete");
    let mut host = open(&dir, 901);
    let session = begin(&mut host, "agent:builder");
    call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"doc":"ara.project.json","why":"Create a generated level","commands":[{"cmd":"doc.create","args":{"path":"levels/generated.scene","id":"sc_generated"}}]}),
    );
    call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"doc":"levels/generated.scene","why":"Populate the generated level","commands":[
            {"cmd":"entity.create","args":{"id":"e_root","components":{"Transform":{}}}},
            {"cmd":"entity.create","args":{"id":"e_child","parent":"e_root","components":{"Transform":{"position":[1,2,3]}}}}
        ]}),
    );
    host.flush();
    let before = fs::read_to_string(dir.join("content/levels/generated.scene")).expect("saved scene");
    call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"doc":"ara.project.json","why":"Regenerate the level through document ops","commands":[{"cmd":"doc.delete","args":{"document":"@#sc_generated"}}]}),
    );
    host.flush();
    assert!(!dir.join("content/levels/generated.scene").exists());
    drop(host);
    let mut host = open(&dir, 902);
    let session = begin(&mut host, "agent:builder");
    assert!(!dir.join("content/levels/generated.scene").exists());
    call(&mut host, "history.undo", json!({"session":session}));
    host.flush();
    assert_eq!(
        before,
        fs::read_to_string(dir.join("content/levels/generated.scene")).expect("restored scene")
    );
}

fn scene(dir: &Path) -> String {
    fs::read_to_string(dir.join("content/levels/arena.scene")).expect("scene file")
}

fn name_of(h: &mut Host, s: &str, entity: &str) -> Value {
    let r = call(
        h,
        "inspect",
        json!({ "session": s, "doc": "levels/arena.scene", "entities": [entity], "format": "json" }),
    );
    r["entities"][0]["name"].clone()
}

#[test]
fn json_rpc_round_trips_and_errors_carry_diagnostics() {
    let dir = project("rpc");
    let mut h = open(&dir, 1);
    let reply = |h: &mut Host, text: &str| -> Value {
        serde_json::from_str(&h.handle_text(ConnId::LOCAL, text).expect("a response"))
            .expect("JSON")
    };
    let r = reply(
        &mut h,
        r#"{"jsonrpc":"2.0","id":1,"method":"rpc.discover"}"#,
    );
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["protocol"], "ara-ep/1");
    assert!(
        r["result"]["methods"]
            .as_array()
            .expect("methods")
            .iter()
            .any(|m| m["name"] == "cmd.apply" && m["params"]["properties"]["commands"].is_object())
    );

    let r = reply(
        &mut h,
        r#"{"jsonrpc":"2.0","id":"a","method":"sesion.begin","params":{}}"#,
    );
    assert_eq!(r["error"]["code"], -32601);
    let d = &r["error"]["data"]["diagnostics"][0];
    assert_eq!(d["code"], "ARA1802");
    assert!(
        d["message"]
            .as_str()
            .is_some_and(|m| m.contains("`session.begin`"))
    );

    let r = reply(
        &mut h,
        r#"{"jsonrpc":"2.0","id":2,"method":"play.start","params":{}}"#,
    );
    assert_eq!(r["error"]["data"]["diagnostics"][0]["code"], "ARA1806");

    let d = fail(
        &mut h,
        "query",
        json!({ "session": "ses_nope", "q": "Light" }),
    );
    assert_eq!(d.code, codes::SESSION_NOT_FOUND);
    assert!(
        matches!(&d.fixes[0].action, FixAction::Call { method, .. } if method == "session.begin")
    );

    let s = begin(&mut h, "human");
    let d = fail(&mut h, "query", json!({ "session": s, "qq": "Light" }));
    assert_eq!(
        (d.code, d.location.path.as_deref()),
        (codes::UNKNOWN_FIELD, Some("params.qq"))
    );
    assert_eq!(
        h.handle_text(
            ConnId::LOCAL,
            r#"{"jsonrpc":"2.0","method":"rpc.discover"}"#
        ),
        None,
        "notifications get no response"
    );
}

#[test]
fn scopes_are_granted_by_policy_and_enforced_before_running() {
    let dir = project("scopes");
    let mut h = open(&dir, 2);
    let r = call(
        &mut h,
        "session.begin",
        json!({ "client": "claude-code", "scopes": ["read", "net"] }),
    );
    assert_eq!(r["author"], "agent:claude-code");
    assert_eq!(
        (r["granted"].clone(), r["denied"].clone()),
        (json!(["read"]), json!(["net"]))
    );
    let s = r["session"].as_str().expect("handle").to_owned();

    let d = fail(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "commands": [{ "cmd": "entity.create" }] }),
    );
    assert_eq!(
        (d.code, d.location.command),
        (codes::MISSING_SCOPE, Some(0)),
        "the registry checks the command's scope"
    );
    let d = fail(&mut h, "history.undo", json!({ "session": s }));
    assert_eq!(
        d.code,
        codes::MISSING_SCOPE,
        "the host checks the method's scope"
    );
    assert!(d.message.contains("`edit:scene`"), "{}", d.message);

    let d = fail(
        &mut h,
        "session.begin",
        json!({ "client": "x", "scopes": ["edit:scen"] }),
    );
    assert_eq!(d.code, codes::UNKNOWN_SCOPE);

    let agent = call(&mut h, "session.begin", json!({ "client": "codex" }));
    let granted: Vec<Scope> = serde_json::from_value(agent["granted"].clone()).expect("scopes");
    assert_eq!(granted, Scope::agent_default());
    let logs = call(
        &mut h,
        "logs",
        json!({ "session": s, "codes": ["ARA1805"] }),
    );
    assert_eq!(logs["total"], 1, "the denial is recorded");
}

#[test]
fn dry_run_equals_apply_and_symbols_map_to_real_ids() {
    let dir = project("dryrun");
    let mut h = open(&dir, 3);
    let s = begin(&mut h, "agent:claude");
    let batch = |dry: bool| {
        json!({
            "session": s, "doc": "levels/arena.scene", "why": "Add a crate on a new pad", "dryRun": dry,
            "commands": [
                { "cmd": "entity.create", "as": "$pad", "args": { "name": "Pad", "components": { "Transform": {} } } },
                { "cmd": "prefab.instantiate", "as": "$crate", "args": { "prefab": "@prefabs/crate.prefab", "parent": "$pad", "position": [0, 0.5, 0] } },
                { "cmd": "transform.set", "args": { "entities": ["$pad"], "position": [2, 0, 3] } }
            ]
        })
    };
    let dry = call(&mut h, "cmd.apply", batch(true));
    assert_eq!(
        (dry["dryRun"].clone(), dry["revision"].clone()),
        (json!(true), json!(0))
    );
    let real = call(&mut h, "cmd.apply", batch(false));
    for key in ["ids", "diff", "ops", "outputs"] {
        assert_eq!(
            dry[key], real[key],
            "{key} differs between dry run and apply"
        );
    }
    let pad = real["ids"]["$pad"]
        .as_str()
        .expect("$pad mapped")
        .to_owned();
    assert!(pad.starts_with("e_"), "{pad}");
    assert_eq!(real["diff"]["created"].as_array().map(Vec::len), Some(2));
    assert!(
        real["checkpoint"]
            .as_str()
            .is_some_and(|c| c.starts_with("cp_")),
        "agent turns checkpoint"
    );
    assert_eq!(name_of(&mut h, &s, &pad), "Pad");

    let d = fail(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "commands": [{ "cmd": "transform.set", "as": "$x", "args": { "entities": ["e_flr1"] } }] }),
    );
    assert_eq!(d.location.path.as_deref(), Some("commands[0].as"));
    let d = fail(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "commands": [{ "cmd": "transform.set", "args": { "entities": ["e_flr1"], "position": [1, 2] } }] }),
    );
    assert_eq!(
        d.location.path.as_deref(),
        Some("commands[0].args.position")
    );
}

#[test]
fn entity_revert_works_for_human_authored_cli_history_too() {
    let dir = project("human-entity-revert");
    let mut h = open(&dir, 414);
    let human = begin(&mut h, "human");
    let change = call(
        &mut h,
        "cmd.apply",
        json!({"session":human,"commands":[
            {"cmd":"property.set","args":{"entity":"e_flr1","path":"name","value":"Ground"}}
        ]}),
    );
    let result = call(
        &mut h,
        "review.revert",
        json!({"session":human,"entity":"e_flr1"}),
    );
    assert_eq!(result["reverted"][0]["txn"], change["txn"]);
    assert_eq!(result["reverted"][0]["author"], "human");
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Floor");
}

#[test]
fn partial_entity_reverts_do_not_become_new_agent_turns_after_restart() {
    for (index, who) in ["agent:builder", "human"].into_iter().enumerate() {
        let dir = project(&format!("partial-revert-restart-{index}"));
        let mut h = open(&dir, 412);
        let agent = begin(&mut h, who);
        let made = call(
            &mut h,
            "cmd.apply",
            json!({"session":agent,"why":"two entity turn","commands":[
                {"cmd":"property.set","args":{"entity":"e_flr1","path":"name","value":"Ground"}},
                {"cmd":"entity.create","as":"marker","args":{"name":"Marker"}}
            ]}),
        );
        let marker = made["ids"]["$marker"].as_str().expect("marker").to_owned();
        call(
            &mut h,
            "review.revert",
            json!({"session":agent,"entity":marker}),
        );
        drop(h);
        let mut h = open(&dir, 413);
        let agent = begin(&mut h, who);
        assert_eq!(
            fail(
                &mut h,
                "review.revert",
                json!({"session":agent,"entity":marker})
            )
            .code,
            codes::NOT_REVIEWABLE,
            "a compensating transaction is not another authoring turn"
        );
        assert_eq!(name_of(&mut h, &agent, "e_flr1"), "Ground");
    }
}

#[test]
fn entity_revert_selects_the_calling_agents_latest_turn_and_reports_attribution() {
    let dir = project("entity-latest-turn");
    let mut h = open(&dir, 410);
    let agent = begin(&mut h, "agent:builder");
    let other = begin(&mut h, "agent:lighting");
    let edit = |h: &mut Host, session: &str, name: &str| {
        call(
            h,
            "cmd.apply",
            json!({
                "session":session,"why":name,"commands":[{"cmd":"property.set","args":{
                    "entity":"e_flr1","path":"name","value":name
                }}]
            }),
        )
    };
    edit(&mut h, &agent, "Ground");
    let latest = edit(&mut h, &agent, "Recent ground");
    call(
        &mut h,
        "cmd.apply",
        json!({"session":other,"why":"light separately","commands":[
            {"cmd":"property.set","args":{"entity":"e_flr1","path":"Transform.position","value":[1,0,0]}}
        ]}),
    );
    call(&mut h, "review.accept", json!({"session":agent,"all":true}));
    drop(h);
    let mut h = open(&dir, 411);
    let agent = begin(&mut h, "agent:builder");
    let result = call(
        &mut h,
        "review.revert",
        json!({"session":agent,"entity":"e_flr1"}),
    );
    assert_eq!(
        result["reverted"].as_array().map(Vec::len),
        Some(1),
        "{result}"
    );
    assert_eq!(result["reverted"][0]["txn"], latest["txn"]);
    assert_eq!(result["reverted"][0]["author"], "agent:builder");
    assert!(
        result["text"]
            .as_str()
            .is_some_and(|s| s.contains("Recent ground"))
    );
    assert_eq!(name_of(&mut h, &agent, "e_flr1"), "Ground");
    let inspected = call(
        &mut h,
        "inspect",
        json!({"session":agent,"entities":["e_flr1"],"format":"json"}),
    );
    assert!(
        inspected.to_string().contains("1.0") || inspected.to_string().contains("[1,0,0]"),
        "{inspected}"
    );
    call(&mut h, "review.accept", json!({"session":agent,"all":true}));
}

#[test]
fn review_accepts_and_reverts_per_transaction_and_entity() {
    let dir = project("review");
    let mut h = open(&dir, 4);
    let agent = begin(&mut h, "agent:claude");
    let human = begin(&mut h, "human");
    let r = call(
        &mut h,
        "cmd.apply",
        json!({ "session": agent, "doc": "levels/arena.scene", "why": "Rename and add a marker",
                "commands": [
                    { "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Ground" } },
                    { "cmd": "entity.create", "as": "$m", "args": { "name": "Marker" } }
                ] }),
    );
    let txn = r["txn"].as_str().expect("txn").to_owned();
    let marker = r["ids"]["$m"].as_str().expect("marker").to_owned();
    let pending = call(&mut h, "review.list", json!({ "session": human }));
    assert_eq!(pending["items"], json!([]), "D24 has no pending review");

    call(
        &mut h,
        "review.accept",
        json!({ "session": agent, "txn": txn }),
    );

    call(
        &mut h,
        "review.revert",
        json!({ "session": human, "txn": txn, "entity": "e_flr1" }),
    );
    assert_eq!(name_of(&mut h, &human, "e_flr1"), "Floor");
    assert_eq!(
        name_of(&mut h, &human, &marker),
        "Marker",
        "the other entity stays"
    );
    let history = call(
        &mut h,
        "history.list",
        json!({ "session": human, "limit": 5 }),
    );
    let original = history["items"]
        .as_array()
        .and_then(|items| items.iter().find(|i| i["txn"] == txn.as_str()))
        .expect("listed")
        .clone();
    assert_eq!(
        original["status"], "accepted",
        "the marker remains accepted"
    );

    call(
        &mut h,
        "review.accept",
        json!({ "session": human, "txn": txn }),
    );
    let accepted = call(
        &mut h,
        "review.list",
        json!({ "session": human, "status": "accepted" }),
    );
    assert_eq!(
        accepted["items"][0]["review"][format!("sc_arena001/{marker}")],
        "accepted"
    );
    assert_eq!(
        accepted["items"][0]["review"]["sc_arena001/e_flr1"],
        "reverted"
    );
    call(
        &mut h,
        "review.accept",
        json!({ "session": human, "all": true }),
    );
}

#[test]
fn rollback_returns_to_a_turn_checkpoint_all_or_nothing() {
    let dir = project("rollback");
    let original = scene(&dir);
    let mut h = open(&dir, 5);
    let s = begin(&mut h, "agent:claude");
    let mut checkpoints = Vec::new();
    for name in ["One", "Two"] {
        let r = call(
            &mut h,
            "cmd.apply",
            json!({ "session": s, "doc": "sc_arena001", "why": format!("rename to {name}"),
                    "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": name } }] }),
        );
        checkpoints.push(r["checkpoint"].as_str().expect("checkpoint").to_owned());
    }
    let diff = call(
        &mut h,
        "session.diff",
        json!({ "session": s, "checkpoint": checkpoints[1] }),
    );
    assert_eq!(diff["text"], "~ sc_arena001/e_flr1 name: One → Two\n");
    let r = call(
        &mut h,
        "session.rollback",
        json!({ "session": s, "to": checkpoints[0] }),
    );
    assert_eq!(r["reverted"].as_array().map(Vec::len), Some(2));
    h.flush();
    assert_eq!(scene(&dir), original, "back to the file as it was");
    let d = fail(
        &mut h,
        "session.rollback",
        json!({ "session": s, "to": "cp_nope" }),
    );
    assert_eq!(d.code, codes::CHECKPOINT_NOT_FOUND);
}

#[test]
fn the_journal_replays_commits_lost_in_a_crash() {
    let dir = project("crash");
    let original = scene(&dir);
    let mut h = open(&dir, 6);
    let s = begin(&mut h, "agent:claude");
    let r = call(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "why": "rename the floor",
                "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Ground" } }] }),
    );
    let txn = r["txn"].as_str().expect("txn").to_owned();
    h.kill();
    assert_eq!(scene(&dir), original, "the crash lost the unflushed write");

    let mut h = open(&dir, 60);
    assert!(
        scene(&dir).contains("\"Ground\""),
        "recovered and flushed on open"
    );
    let logs = call(
        &mut h,
        "logs",
        json!({ "session": s, "codes": ["ARA1906"] }),
    );
    assert_eq!(logs["total"], 1);
    let history = call(&mut h, "history.list", json!({ "session": s }));
    assert_eq!(
        history["items"][0]["txn"],
        txn.as_str(),
        "same transaction ID"
    );
    assert_eq!(
        history["items"][0]["status"], "accepted",
        "review state survives"
    );
    call(&mut h, "history.undo", json!({ "session": s }));
    drop(h);
    assert_eq!(scene(&dir), original, "undo works across the restart");

    let h = open(&dir, 61);
    assert_eq!(h.store().revision(), 2, "nothing replays twice");
}

#[test]
fn external_edits_merge_in_or_are_reported_without_overwriting() {
    let dir = project("external");
    let mut h = open(&dir, 7);
    let s = begin(&mut h, "agent:claude");

    fs::write(
        dir.join("content/levels/arena.scene"),
        scene(&dir).replace("\"Floor\"", "\"Ground floor\""),
    )
    .expect("edit");
    h.tick();
    assert_eq!(name_of(&mut h, &s, "e_flr1"), "Ground floor");
    let history = call(&mut h, "history.list", json!({ "session": s, "limit": 1 }));
    assert_eq!(history["items"][0]["label"], "external edit");
    assert_eq!(history["items"][0]["author"], "human");

    // An unsaved agent change, then a conflicting edit on disk.
    call(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "why": "rename the floor",
                "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Agent" } }] }),
    );
    let human_text = scene(&dir).replace("\"Ground floor\"", "\"Human\"");
    fs::write(dir.join("content/levels/arena.scene"), &human_text).expect("edit");
    h.tick();
    let logs = call(
        &mut h,
        "logs",
        json!({ "session": s, "codes": ["ARA1908"] }),
    );
    assert_eq!(logs["total"], 1, "{logs}");
    assert_eq!(name_of(&mut h, &s, "e_flr1"), "Agent", "memory kept");
    h.flush();
    assert_eq!(scene(&dir), human_text, "the file is not overwritten");
    let status = call(&mut h, "session.status", json!({ "session": s }));
    assert_eq!(
        status["files"]["diverged"]["levels/arena.scene"],
        "conflict"
    );

    call(
        &mut h,
        "doc.sync",
        json!({ "session": s, "doc": "levels/arena.scene", "keep": "memory" }),
    );
    assert!(scene(&dir).contains("\"Agent\""));

    fs::write(dir.join("content/levels/arena.scene"), "{ not json").expect("break");
    h.tick();
    let logs = call(
        &mut h,
        "logs",
        json!({ "session": s, "codes": ["ARA1909"] }),
    );
    assert_eq!(logs["total"], 1);
    assert_eq!(name_of(&mut h, &s, "e_flr1"), "Agent");
}

/// Tick until `done`, failing after ten seconds.
fn tick_until(h: &mut Host, what: &str, mut done: impl FnMut(&mut Host) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(h) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        h.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// With its own cadence (OS notifications, or polling without them) the file
/// thread saves and watches off the tick: the host's saves never come back as
/// external edits, and real edits and new documents still arrive.
#[test]
fn the_file_thread_saves_and_watches_off_the_tick() {
    let dir = project("file-thread");
    let config = HostConfig {
        debounce: Duration::from_millis(20),
        seed: Some(70),
        ..HostConfig::default()
    };
    let mut h = Host::open(Project::open(&dir).expect("project"), config).expect("host");
    let s = begin(&mut h, "agent:claude");
    call(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "doc": "levels/arena.scene", "why": "rename the floor",
                "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "Saved" } }] }),
    );
    tick_until(&mut h, "the debounced save", |_| {
        scene(&dir).contains("\"Saved\"")
    });
    // Long enough for the thread to see its own write.
    for _ in 0..30 {
        h.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    let merged = |h: &mut Host| {
        call(h, "logs", json!({ "session": s, "codes": ["ARA1907"] }))["total"].clone()
    };
    assert_eq!(
        merged(&mut h),
        0,
        "the host's own save came back as an edit"
    );

    fs::write(
        dir.join("content/levels/arena.scene"),
        scene(&dir).replace("\"Saved\"", "\"Edited\""),
    )
    .expect("edit");
    tick_until(&mut h, "the external edit", |h| {
        name_of(h, &s, "e_flr1") == "Edited"
    });
    assert_eq!(merged(&mut h), 1);

    fs::write(
        dir.join("content/levels/extra.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_extra001","records":{}}"#,
    )
    .expect("new document");
    tick_until(&mut h, "the new document", |h| {
        h.store().workspace().find("levels/extra.scene").is_some()
    });
}

#[test]
fn query_and_inspect_speak_scene_to_text() {
    let dir = project("text");
    let mut h = open(&dir, 8);
    let s = begin(&mut h, "human");
    let summary = call(
        &mut h,
        "inspect",
        json!({ "session": s, "doc": "levels/arena.scene", "depth": 9 }),
    );
    insta::assert_snapshot!("inspect_summary", summary["text"].as_str().expect("text"));
    let details = call(
        &mut h,
        "inspect",
        json!({ "session": s, "doc": "levels/arena.scene", "entities": ["e_lmp1", "e_stk2/e_c002"] }),
    );
    insta::assert_snapshot!("inspect_details", details["text"].as_str().expect("text"));

    let q = call(
        &mut h,
        "query",
        json!({ "session": s, "q": "MeshRenderer under:e_stk2", "fields": ["MeshRenderer.tint"], "limit": 2 }),
    );
    insta::assert_snapshot!("query_page", q["text"].as_str().expect("text"));
    assert_eq!(q["total"], 3);
    let next = call(
        &mut h,
        "query",
        json!({ "session": s, "q": "MeshRenderer under:e_stk2", "cursor": q["next"], "detail": "handles" }),
    );
    // `e_c002/e_lid1` is removed by the instance, so the last match is the crate.
    assert_eq!(
        next["items"],
        json!([{ "entity": "e_stk2/e_c002", "doc": "sc_arena001" }])
    );
    let lights = call(
        &mut h,
        "query",
        json!({ "session": s, "filter": { "where": [{ "path": "Light.intensity", "op": "gt", "value": 500 }] } }),
    );
    assert_eq!(lights["items"][0]["entity"], "e_lmp1");
}

/// Params of `play.step` in the test service.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StepParams {
    /// Session handle.
    #[allow(dead_code)] // Checked by the host before the call.
    session: String,
    /// Ticks to advance.
    ticks: u32,
}

struct Play;

impl Service for Play {
    fn name(&self) -> &'static str {
        "play"
    }

    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<StepParams>(db, "play.step", "Advance the play world.")
                .scope(Scope::Play)
                .action("ara_play", "step"),
        ]
    }

    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        _method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        let p: StepParams = ara_host::parse(params)?;
        let job = cx.start_job("play.step");
        cx.finish_job(&job, Ok(json!(p.ticks)));
        Ok(json!({ "tick": p.ticks, "author": cx.session().map(|s| s.author.to_string()) }))
    }
}

#[test]
fn services_register_methods_that_light_up_immediately() {
    let dir = project("service");
    let mut h = open(&dir, 9);
    let s = begin(&mut h, "agent:claude");
    let d = fail(&mut h, "play.step", json!({ "session": s, "ticks": 3 }));
    assert_eq!(d.code, codes::SERVICE_UNAVAILABLE);

    h.register(Box::new(Play)).expect("registers");
    let r = call(&mut h, "play.step", json!({ "session": s, "ticks": 3 }));
    assert_eq!(r, json!({ "tick": 3, "author": "agent:claude" }));
    let d = fail(&mut h, "play.step", json!({ "session": s, "ticks": -1 }));
    assert_eq!(
        d.code,
        codes::OUT_OF_RANGE,
        "params are validated before the service runs"
    );
    let info = h
        .catalog()
        .iter()
        .find(|m| m.name == "play.step")
        .expect("in the catalog");
    assert_eq!(info.service, "play");
    assert_eq!(
        info.params["properties"]["ticks"]["description"],
        "Ticks to advance."
    );
    assert!(
        h.register(Box::new(Play)).is_err(),
        "method names are unique"
    );
    let jobs = call(&mut h, "jobs.list", json!({ "session": s }));
    assert_eq!(jobs["jobs"][0]["result"], 3);
}

/// Params of `test.spawn`.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpawnParams {
    /// Entities to create.
    count: u32,
}

/// A registry job that creates entities and reports progress after each.
struct Spawn;

impl ara_cmd::Job for Spawn {
    type Params = SpawnParams;
    type Output = u32;
    const NAME: &'static str = "test.spawn";
    const SUMMARY: &'static str = "Create entities, reporting progress.";

    fn examples() -> Vec<Value> {
        vec![json!({ "count": 2 })]
    }

    fn run(ctx: &mut ara_cmd::JobCtx<'_, '_>, p: SpawnParams) -> Result<u32, Diagnostic> {
        for i in 0..p.count {
            let id = ctx.edit.new_id(None, "e")?;
            ctx.edit.create(id, ara_doc::Record::new())?;
            ctx.progress((i + 1) as f32 / p.count as f32, "spawned");
        }
        Ok(p.count)
    }
}

#[test]
fn subscribers_get_ops_history_and_job_progress() {
    let dir = project("subscribe");
    let mut project = Project::open(&dir).expect("project");
    project.registry.job::<Spawn>(&mut project.db);
    let config = HostConfig {
        seed: Some(10),
        ..HostConfig::default()
    };
    let mut h = Host::open(project, config).expect("host");
    let s = begin(&mut h, "human");
    let conn = ConnId(7);
    h.call(
        conn,
        "subscribe",
        json!({ "session": s, "topics": ["ops", "history", "jobs"] }),
    )
    .expect("subscribed");
    let job = call(
        &mut h,
        "jobs.start",
        json!({ "session": s, "doc": "levels/arena.scene", "job": "test.spawn", "args": { "count": 2 } }),
    );
    assert_eq!(
        (job["status"].clone(), job["result"]["output"].clone()),
        (json!("done"), json!(2))
    );

    let notes: Vec<Value> = h
        .drain_outbox()
        .into_iter()
        .filter(|(c, _)| *c == conn)
        .map(|(_, n)| n)
        .collect();
    let of =
        |method: &str| -> Vec<&Value> { notes.iter().filter(|n| n["method"] == method).collect() };
    let progress: Vec<Value> = of("jobs")
        .iter()
        .map(|n| n["params"]["progress"].clone())
        .collect();
    assert_eq!(
        progress,
        [json!(0.0), json!(0.5), json!(1.0), json!(1.0)],
        "start, two steps, done"
    );
    let ops = of("ops");
    assert_eq!(ops.len(), 1, "one transaction");
    assert_eq!(
        ops[0]["params"]["txn"]["ops"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(of("history")[0]["params"]["label"], "test.spawn");

    h.call(
        conn,
        "unsubscribe",
        json!({ "session": s, "topics": ["ops", "history", "jobs"] }),
    )
    .expect("unsubscribed");
    call(
        &mut h,
        "jobs.start",
        json!({ "session": s, "doc": "levels/arena.scene", "job": "test.spawn", "args": { "count": 1 } }),
    );
    assert!(h.drain_outbox().is_empty());
}

#[test]
fn calls_find_their_document_and_report_created_ids_plainly() {
    let dir = project("infer");
    let mut h = open(&dir, 11);
    let s = begin(&mut h, "agent:claude");
    let begun = call(&mut h, "session.begin", json!({ "client": "codex" }));
    assert_eq!(
        begun["documents"].as_array().map(Vec::len),
        Some(4),
        "scene, two prefabs and project settings"
    );

    // No `doc`: the entity named by its selector picks the scene; `as` may repeat `id`.
    let r = call(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "why": "A pad on the floor",
                "commands": [
                    { "cmd": "entity.create", "as": "$pad", "args": { "id": "$pad", "name": "Pad", "parent": "name:Floor" } },
                    { "cmd": "property.set", "args": { "entity": "name:Floor", "path": "name", "value": "Ground" } }
                ] }),
    );
    let pad = r["ids"]["$pad"].as_str().expect("pad").to_owned();
    assert_eq!(r["doc"], "levels/arena.scene");
    let text = r["text"].as_str().expect("text");
    assert!(
        text.starts_with("applied tx_") && text.contains("to levels/arena.scene"),
        "{text}"
    );
    assert!(
        text.contains(&format!("created: {pad} Pad ($pad)")),
        "{text}"
    );
    assert!(text.contains("modified: e_flr1 Ground"), "{text}");

    // `as` without its `$`; `schema.describe` with one `name`.
    let r = call(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "why": "A marker", "commands": [
            { "cmd": "entity.create", "as": "marker", "args": { "name": "Marker" } },
            { "cmd": "property.set", "args": { "entity": "$marker", "path": "name", "value": "Mark" } }
        ] }),
    );
    assert!(r["ids"]["$marker"].is_string(), "{r}");
    let d = call(
        &mut h,
        "schema.describe",
        json!({ "session": s, "name": "Transform" }),
    );
    assert_eq!(d["items"][0]["name"], "Transform");

    // Lenient document names, name selectors and the main-scene default for reads.
    let one = call(
        &mut h,
        "inspect",
        json!({ "session": s, "doc": "arena", "entities": ["name:Pad"] }),
    );
    assert!(
        one["text"]
            .as_str()
            .is_some_and(|t| t.starts_with(&format!("{pad} Pad"))),
        "{one}"
    );
    let all = call(&mut h, "inspect", json!({ "session": s }));
    assert!(
        all["text"]
            .as_str()
            .is_some_and(|t| t.contains("levels/arena.scene"))
    );

    // A dry run says what it would do.
    let dry = call(
        &mut h,
        "cmd.apply",
        json!({ "session": s, "dryRun": true, "commands": [{ "cmd": "entity.delete", "args": { "entities": [pad] } }] }),
    );
    let text = dry["text"].as_str().expect("text");
    assert!(
        text.contains("nothing was applied") && text.contains(&format!("would delete: {pad} Pad")),
        "{text}"
    );
}

#[test]
fn agent_changes_need_a_why_and_get_every_problem_at_once() {
    let dir = project("why");
    let mut h = open(&dir, 12);
    let s = begin(&mut h, "agent:claude");
    let diags = h
        .call(
            ConnId::LOCAL,
            "cmd.apply",
            json!({ "session": s, "commands": [{ "cmd": "property.set", "args": { "entity": "Floor", "path": "name", "value": "X" } }] }),
        )
        .expect_err("no why")
        .diagnostics();
    let codes_seen: Vec<_> = diags.iter().map(|d| d.code).collect();
    assert_eq!(codes_seen, [codes::TYPE_MISMATCH, codes::MISSING_WHY]);
    assert_eq!(diags[0].fixes[0].title, "Use `e_flr1` (Floor)");
    assert!(
        matches!(&diags[0].fixes[0].action, ara_diag::FixAction::Set { path, .. } if path == "commands[0].args.entity")
    );
    assert_eq!(
        call(&mut h, "history.list", json!({ "session": s }))["items"],
        json!([])
    );
    let human = begin(&mut h, "human");
    call(
        &mut h,
        "cmd.apply",
        json!({ "session": human, "commands": [{ "cmd": "property.set", "args": { "entity": "e_flr1", "path": "name", "value": "X" } }] }),
    );
}
