//! Native file history must retain its loaded model and references at every boundary.
use super::*;

fn model(host: &Host, logical: &str) {
    let store = host.store();
    let workspace = store.workspace();
    let expected = format!("{}/{logical}", ara_doc::AUTHORING_ROOT);
    let prefab = workspace
        .doc("pf_prop")
        .expect("the live prefab must not disappear after file history");
    assert_eq!(workspace.path_of("pf_prop"), Some(expected.as_str()));
    assert_eq!(
        prefab.record("e_prop").expect("prefab record")["name"],
        "Prop"
    );
    let main = workspace.doc("sc_main").expect("main");
    assert_eq!(
        main.record("e_instance").expect("instance")["prefab"],
        format!("@{logical}#pf_prop")
    );
    let mut diagnostics = Vec::new();
    let resolved = ara_doc::prefab::resolve(
        main,
        store.schema(),
        workspace,
        &ara_diag::Location::default(),
        &mut diagnostics,
    );
    assert!(
        !ara_diag::has_errors(&diagnostics),
        "live prefab resolution failed: {diagnostics:?}"
    );
    assert!(
        resolved
            .values()
            .any(|entity| entity.record.get("name").is_some_and(|name| name == "Prop"))
    );
}

fn finish_with_model(
    host: &mut Host,
    session: &str,
    response: Value,
    before: u64,
    old: &str,
    new: &str,
) {
    let job = response["job"].as_str().map(str::to_owned);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.tick();
        let revision = host.store().revision();
        assert!(
            revision == before || revision == before + 1,
            "only the requested history transaction may advance authority"
        );
        model(host, if revision == before { old } else { new });
        let done = if let Some(job) = &job {
            let status = call(host, "jobs.status", json!({"session":session,"job":job}));
            assert_ne!(status["status"], "failed", "{status}");
            status["status"] == "done"
        } else {
            true
        };
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "real asset history did not finish"
        );
        std::thread::yield_now();
    }
    assert_eq!(host.store().revision(), before + 1);
    model(host, new);
    host.flush();
    model(host, new);
    assert!(!host.unsaved());
}

#[test]
fn real_asset_rename_undo_redo_preserves_model_path_and_prefab_reference_before_and_after_flush() {
    let scratch = scratch();
    write_project(scratch.path());
    let authored = scratch.path().join(ara_doc::AUTHORING_ROOT);
    fs::write(
        authored.join("props/A.prefab"),
        ara_json::to_canonical_string(&json!({
            "$schema":"ara://schema/prefab/1","id":"pf_prop","records":{
                "e_prop":{"name":"Prop","Transform":{},"order":"a0"}
            }
        })),
    )
    .expect("prefab fixture");
    fs::write(
        authored.join("scenes/main.scene"),
        ara_json::to_canonical_string(&json!({
            "$schema":"ara://schema/scene/1","id":"sc_main","records":{
                "e_instance":{"prefab":"@props/A.prefab#pf_prop","Transform":{},"order":"a0"}
            }
        })),
    )
    .expect("referencing scene");
    let mut host = Host::open(
        Project::open(scratch.path()).expect("project"),
        HostConfig {
            debounce: Duration::from_secs(3600),
            watch_every: Duration::from_secs(3600),
            scan_every: Duration::from_secs(3600),
            seed: Some(518),
            ..HostConfig::default()
        },
    )
    .expect("host");
    host.register(Box::new(AssetService::new(
        ProcessWorker::new(scratch.path().join("must-not-run-importer")),
        Arc::new(|_, _| Ok(())),
    )))
    .expect("asset service");
    let session = begin(&mut host, None);
    host.flush();
    model(&host, "props/A.prefab");
    let before = host.store().revision();
    let renamed = call(
        &mut host,
        "cmd.apply",
        json!({"session":session,"doc":"ara.project.json","why":"Native rename model regression",
        "commands":[{"cmd":"asset.rename","args":{"source":"props/A.prefab","name":"Renamed.prefab"}}]}),
    );
    finish_with_model(
        &mut host,
        &session,
        renamed,
        before,
        "props/A.prefab",
        "props/Renamed.prefab",
    );
    for (method, old, new) in [
        ("history.undo", "props/Renamed.prefab", "props/A.prefab"),
        ("history.redo", "props/A.prefab", "props/Renamed.prefab"),
    ] {
        let before = host.store().revision();
        let result = call(&mut host, method, json!({"session":session}));
        finish_with_model(&mut host, &session, result, before, old, new);
    }
    assert!(authored.join("props/Renamed.prefab").is_file());
    assert!(!authored.join("props/A.prefab").exists());
}
