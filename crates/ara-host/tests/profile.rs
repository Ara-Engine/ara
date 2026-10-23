#![allow(clippy::unwrap_used)]
use ara_host::{Host, HostConfig, Project};
use ara_proto::ConnId;
use serde_json::{Value, json};
use std::{fs, path::Path};

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn rpc(host: &mut Host, method: &str, params: Value) -> Value {
    let text = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string();
    serde_json::from_str(&host.handle_text(ConnId::LOCAL, &text).unwrap()).unwrap()
}
#[test]
fn typed_profile_service_round_trips_through_engine_protocol() {
    let root = std::env::temp_dir().join(format!("ara-profile-protocol-{}", std::process::id()));
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project"),
        &root,
    );
    let mut host = Host::open(Project::open(&root).unwrap(), HostConfig::default()).unwrap();
    for method in ["start", "stop", "summary", "hitches", "compare"] {
        assert!(
            host.catalog()
                .iter()
                .any(|m| m.name == format!("profile.{method}"))
        );
    }
    assert!(
        rpc(&mut host, "profile.start", json!({}))
            .get("error")
            .is_some()
    );
    let begin = rpc(
        &mut host,
        "session.begin",
        json!({"client":"perf-agent","author":"agent:perf"}),
    );
    let session = begin["result"]["session"].as_str().unwrap();
    assert_eq!(
        rpc(&mut host, "profile.start", json!({"session":session}))["result"]["recording"],
        true
    );
    for _ in 0..3 {
        host.tick();
    }
    let summary = rpc(
        &mut host,
        "profile.summary",
        json!({"session":session,"topN":100,"limit":0}),
    );
    assert_eq!(summary["result"]["frames"], 3);
    assert_eq!(
        summary["result"]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["key"] == "cpu/host.tick"),
        ara_profile::scope_backend() == "native"
    );
    let baseline = summary["result"].to_string();
    let compared = rpc(
        &mut host,
        "profile.compare",
        json!({"session":session,"baseline":baseline,"current":baseline}),
    );
    assert_eq!(compared["result"]["passed"], true);
    assert!(
        rpc(
            &mut host,
            "profile.hitches",
            json!({"session":session,"limit":2})
        )
        .get("result")
        .is_some()
    );
    assert_eq!(
        rpc(&mut host, "profile.stop", json!({"session":session}))["result"]["recording"],
        false
    );
    host.tick();
    assert_eq!(host.profiler().profile().frames.len(), 3);
    assert!(
        rpc(
            &mut host,
            "profile.summary",
            json!({"session":session,"maxBytes":1})
        )
        .get("error")
        .is_some()
    );
    drop(host);
    fs::remove_dir_all(root).unwrap();
}
