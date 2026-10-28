//! Exercise the read-only path contracts before layout activation is merged.
use super::*;
use ara_doc::{AssetId, Document, Workspace};
use ara_ops::Store;
use ara_why::Facts;

#[test]
fn why_resolves_authored_scene_sources_and_preserves_project_policy_storage_paths() {
    let root = std::env::temp_dir().join(format!("ara-why-paths-{}", std::process::id()));
    let authored = root.join(ara_doc::AUTHORING_ROOT);
    fs::create_dir_all(authored.join("scenes")).expect("scenes");
    fs::create_dir_all(authored.join("materials")).expect("materials");
    fs::create_dir_all(root.join("settings")).expect("project policy");
    fs::write(
        root.join("ara.project.json"),
        r#"{"name":"Why paths","layoutVersion":4,"main":"scenes/main.scene"}"#,
    )
    .expect("marker");
    let source =
        "material Broken : surface budget 1 { output baseColor = definitely_missing_symbol; }\n";
    fs::write(authored.join("materials/broken.material"), source).expect("material");
    fs::write(root.join("settings/project.json"), "project policy\n").expect("policy");
    let project = Project::open_with(&root, |db, registry| {
        let diagnostics = ara_runtime::engine::register_schemas(db);
        ara_runtime::engine::register_commands(registry, db);
        diagnostics
    })
    .expect("project");
    let scene = json!({"$schema":"ara://schema/scene/1","id":"sc_why01","records":{"e_box001":{"name":"Box","MeshRenderer":{}}}});
    fs::write(
        authored.join("scenes/main.scene"),
        ara_json::to_canonical_string(&scene),
    )
    .expect("scene");
    let mut workspace = Workspace::default();
    workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
    let scene = ara_doc::read(&scene.to_string(), "scenes/main.scene", &project.db, None)
        .doc
        .expect("scene document");
    workspace.insert(
        format!("{}/scenes/main.scene", ara_doc::AUTHORING_ROOT),
        scene,
    );
    workspace.insert_with_asset_path(
        format!(
            "{}/materials/broken.material.import.json",
            ara_doc::AUTHORING_ROOT
        ),
        Document::new(
            "import",
            AssetId::parse("gr_broken01").expect("material id"),
        ),
        format!("{}/materials/broken.material", ara_doc::AUTHORING_ROOT),
    );
    workspace.insert_project_document(
        "settings/project.json",
        Document::new("project", AssetId::parse("pj_policy01").expect("policy id")),
    );
    let db = project.db.clone();
    let mut host = Host::open(project, HostConfig::default()).expect("host");
    // Layout loading remains the layout owner's change. Inject the exact workspace
    // that its published contract specifies, without changing default project loading.
    host.st.store = Store::new(db, workspace);
    {
        let facts = crate::why::HostFacts::new(&host.st, None);
        let entity = facts
            .entity("name:Box", Some("scenes/main.scene"))
            .expect("reference scene path");
        assert_eq!(entity.address, "e_box001");
        let asset = facts.asset("@materials/broken.material#gr_broken01");
        assert_eq!(asset.source_exists, Some(true), "{asset:?}");
        assert_eq!(
            facts.read("materials/broken.material").as_deref(),
            Some(source)
        );
        assert_eq!(
            facts.read("settings/project.json").as_deref(),
            Some("project policy\n")
        );
        assert!(facts.read("../outside.material").is_none());
    }
    let session =
        call(&mut host, "session.begin", json!({"client":"why-paths"}))["session"].clone();
    let report = call(
        &mut host,
        "why",
        json!({"session":session,"doc":"scenes/main.scene","question":"name:Box not visible","full":true}),
    );
    assert!(
        !report["causes"]
            .as_array()
            .expect("causes")
            .iter()
            .any(|cause| cause["code"] == "ARA3305"),
        "{report}"
    );
    let material = call(
        &mut host,
        "why",
        json!({"session":session,"question":"@materials/broken.material#gr_broken01","full":true}),
    );
    assert!(
        material.to_string().contains("definitely_missing_symbol"),
        "source diagnosis was not included: {material}"
    );
}
