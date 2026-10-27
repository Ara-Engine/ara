//! Installed providers lower fixes to the ordinary attributed source.replace history.
use ara_host::{Host, HostConfig, Project};
use ara_proto::ConnId;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn complete(host: &mut Host, method: &str, params: Value) -> Value {
    let session = params["session"].clone();
    let result = host.call(ConnId::LOCAL, method, params).expect("call");
    let Some(job) = result["job"].as_str() else {
        return result;
    };
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        host.tick();
        let status = host
            .call(
                ConnId::LOCAL,
                "jobs.status",
                json!({"session":session,"job":job}),
            )
            .expect("job");
        if status["status"] == "done" {
            return status["result"].clone();
        }
        assert_ne!(status["status"], "failed", "{status}");
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn compatibility_source_write_returns_materialized_ops_and_undoable_agent_history() {
    let target = std::env::var_os("ARA_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/source-fixes"));
    let root = target.join(format!("source-write-{}", std::process::id()));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT)).expect("scratch");
    std::fs::write(root.join("ara.project.json"), "{}").expect("marker");
    std::fs::write(
        root.join("main.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_write","records":{}}"#,
    )
    .expect("history document");
    let path = format!("{}/Writer.cs", ara_doc::AUTHORING_ROOT);
    let before = "public sealed class Writer { }";
    let after = "public sealed class Writer { public int Value => 1; }";
    std::fs::write(root.join(&path), before).expect("source");
    let mut host = Host::open(
        Project::open(&root).expect("project"),
        HostConfig::default(),
    )
    .expect("host");
    host.register(Box::new(ara_play_host::PlayService::new(
        &root,
        ara_play_host::PlayConfig::for_project(&root),
    )))
    .expect("play");
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"author":"agent:writer","client":"test"}),
        )
        .expect("session")["session"]
        .clone();
    let result = complete(
        &mut host,
        "script.write",
        json!({"session":session,"path":"Writer.cs","text":after}),
    );
    assert!(result["txn"].is_string(), "{result}");
    assert!(!result["ops"].as_array().expect("ops").is_empty());
    assert_eq!(result["path"], path);
    assert_eq!(result["agent"], true);
    assert_eq!(
        std::fs::read_to_string(root.join(&path)).expect("already materialized"),
        after
    );
    drop(host);
    let mut host =
        Host::open(Project::open(&root).expect("reopen"), HostConfig::default()).expect("host");
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"author":"agent:writer","client":"test"}),
        )
        .expect("session")["session"]
        .clone();
    complete(&mut host, "history.undo", json!({"session":session}));
    host.flush();
    assert_eq!(
        std::fs::read_to_string(root.join(&path)).expect("undone"),
        before
    );
    complete(&mut host, "history.redo", json!({"session":session}));
    host.flush();
    assert_eq!(
        std::fs::read_to_string(root.join(&path)).expect("redone"),
        after
    );
}

#[test]
fn code_fixes_use_prepared_source_ops_and_round_trip_offline_history() {
    let target = std::env::var_os("ARA_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/source-fixes"));
    let root = target.join(format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT)).expect("scratch");
    std::fs::write(root.join("ara.project.json"), "{}").expect("marker");
    std::fs::write(
        root.join(ara_doc::AUTHORING_ROOT).join("main.scene"),
        r#"{"$schema":"ara://schema/scene/1","id":"sc_fix","records":{}}"#,
    )
    .expect("scene");
    let path = format!("{}/Broken.cs", ara_doc::AUTHORING_ROOT);
    let before = "using Ara; public sealed partial class Broken : Behaviour { private int cache; }";
    std::fs::write(root.join(&path), before).expect("source");
    let mut host = Host::open(
        Project::open(&root).expect("project"),
        HostConfig::default(),
    )
    .expect("host");
    host.register(Box::new(ara_play_host::SourceFixService::new(&root)))
        .expect("fix service");
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"author":"agent:fixer","client":"test"}),
        )
        .expect("session")["session"]
        .clone();
    let preview = complete(
        &mut host,
        "source.fix",
        json!({"session":session,"paths":[path],"doc":"sc_fix","apply":false}),
    );
    assert_eq!(preview["commands"][0]["cmd"], "source.replace");
    assert_eq!(
        std::fs::read_to_string(root.join(&path)).expect("untouched"),
        before
    );
    let fixed = complete(
        &mut host,
        "cmd.apply",
        json!({"session":session,"doc":"sc_fix","why":"Apply shipped code fix","commands":preview["commands"]}),
    );
    assert!(fixed["txn"].is_string(), "{fixed}");
    host.flush();
    let after = std::fs::read_to_string(root.join(&path)).expect("fixed");
    assert!(after.contains("[Transient]"), "{after}");
    drop(host);
    let mut host = Host::open(
        Project::open(&root).expect("project"),
        HostConfig::default(),
    )
    .expect("reopen");
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"author":"agent:fixer","client":"test"}),
        )
        .expect("session")["session"]
        .clone();
    complete(&mut host, "history.undo", json!({"session":session}));
    host.flush();
    assert_eq!(
        std::fs::read_to_string(root.join(&path)).expect("undone"),
        before
    );
    drop(host);
    let mut host = Host::open(
        Project::open(&root).expect("project"),
        HostConfig::default(),
    )
    .expect("reopen redo");
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"author":"agent:fixer","client":"test"}),
        )
        .expect("session")["session"]
        .clone();
    complete(&mut host, "history.redo", json!({"session":session}));
    host.flush();
    assert_eq!(
        std::fs::read_to_string(root.join(&path)).expect("redone"),
        after
    );
}
