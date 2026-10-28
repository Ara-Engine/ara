//! Native file history must retain its loaded model and references at every boundary.
use super::*;

fn assert_file_boundary(host: &Host) {
    let history = ara_host::file_state::FileHistory::new(host.root());
    let transaction = host.store().history().last().expect("current transaction");
    let mut states = std::collections::BTreeMap::new();
    for op in &transaction.ops {
        if let ara_doc::Op::File { path, state, .. } = op {
            states.insert(path, state);
        }
    }
    for (path, expected) in states {
        let actual = history.observe(path).expect("physical state");
        if &actual != expected {
            let disk = fs::read_to_string(host.root().join(path)).ok();
            let parsed = disk
                .as_deref()
                .map(|text| ara_doc::read(text, path, host.store().schema(), None));
            let canonical = host
                .store()
                .workspace()
                .docs()
                .find(|(candidate, _)| *candidate == path.as_str())
                .map(|(_, doc)| ara_doc::write(doc, host.store().schema()));
            let detail = json!({"transaction":transaction.id,"revision":transaction.result,"path":path,
                "expected":expected,"actual":actual,"diskText":disk,
                "expectedText":expected.as_ref().map(|state|String::from_utf8_lossy(&history.read(state).expect("immutable snapshot")).into_owned()),
                "memoryCanonical":canonical,"parseWithoutIndex":parsed.as_ref().map(|read|json!({"diagnostics":read.diags,
                    "canonical":read.doc.as_ref().map(|doc|ara_doc::write(doc,host.store().schema()))}))});
            panic!(
                "canonical save changed a committed immutable file snapshot: {}",
                ara_json::to_canonical_string(&detail)
            );
        }
    }
}

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
    assert_file_boundary(host);
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
                "e_instance":{"prefab":"@props/A.prefab#pf_prop","order":"a0"}
            }
        })),
    )
    .expect("referencing scene");
    let project = Project::open(scratch.path()).expect("project");
    for logical in ["props/A.prefab", "scenes/main.scene"] {
        let storage = format!("{}/{logical}", ara_doc::AUTHORING_ROOT);
        let text = fs::read_to_string(scratch.path().join(&storage)).expect("fixture text");
        let read = ara_doc::read(&text, &storage, &project.db, Some(&project.index));
        assert!(
            read.doc.is_some() && !ara_diag::has_errors(&read.diags),
            "the physical fixture must validate before authority opening: {storage}: {:?}",
            read.diags
        );
    }
    let mut host = Host::open(
        project,
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
