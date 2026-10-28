use super::*;
use serde_json::json;
fn document(kind: &str, id: &str, records: Value) -> Document {
    let mut document = Document::new(kind, AssetId::parse(id).expect("asset ID"));
    for (id, record) in records.as_object().expect("records") {
        document.insert_unchecked(
            RecordId::parse(id).expect("record ID"),
            record.as_object().expect("record").clone(),
        );
    }
    document
}
fn source(path: &str, document: Document) -> SourceDoc {
    SourceDoc {
        path: path.to_owned(),
        reference_path: path.to_owned(),
        document,
    }
}
#[test]
fn move_repairs_references_and_only_owner_defined_bare_paths() {
    let prefab = document("prefab", "pf_box", json!({"e_root":{"name":"Box"}}));
    let animation = document("animation", "ac_walk", json!({"an_idle":{}}));
    let sources = vec![
        source("content/kit/box.prefab", prefab),
        source("content/kit/walk.animation", animation),
    ];
    let mappings = Mappings::new("content/kit", Some("content/warehouse"), &sources);
    let mut record=json!({"prefab":"@content/kit/box.prefab#pf_box","animation":"@content/kit/walk.animation#an_idle","main":"content/kit/box.prefab","source":"<image src=\"@content/kit/box.prefab#pf_box\"/>"}).as_object().expect("record").clone();
    mappings.rewrite_record(&document("scene", "sc_main", json!({})), &mut record);
    assert_eq!(record["prefab"], "@content/warehouse/box.prefab#pf_box");
    assert_eq!(
        record["animation"],
        "@content/warehouse/walk.animation#an_idle"
    );
    assert_eq!(
        record["main"], "content/kit/box.prefab",
        "scene fields are not marker paths"
    );
    assert_eq!(
        record["source"],
        "<image src=\"@content/warehouse/box.prefab#pf_box\"/>"
    );
    let mut settings = json!({"settings":{"name":"content/kit","main":"content/kit/box.prefab"}})
        .as_object()
        .expect("settings")
        .clone();
    mappings.rewrite_record(&document("project", "pj_project", json!({})), &mut settings);
    assert_eq!(settings["settings"]["main"], "content/warehouse/box.prefab");
    assert_eq!(settings["settings"]["name"], "content/kit");
}
#[test]
fn duplicate_remaps_nested_prefab_addresses_and_keeps_external_prefab_ids() {
    let prefab = document(
        "prefab",
        "pf_box",
        json!({"e_root":{"name":"Box"},"e_lid":{"parent":"e_root"}}),
    );
    let scene = document(
        "scene",
        "sc_kit",
        json!({"e_instance":{"prefab":"@content/kit/box.prefab#pf_box","overrides":{"e_lid/Transform.position":[0,2,0]},"removed":["e_lid"]},"e_external":{"prefab":"@content/shared/light.prefab#pf_light","overrides":{"e_lid/Light.intensity":5}}}),
    );
    let sources = vec![
        source("content/kit/box.prefab", prefab),
        source("content/kit/main.scene", scene.clone()),
    ];
    let mut mappings = Mappings::new("content/kit", Some("content/copy"), &sources);
    mappings.assets.insert(
        "pf_box".into(),
        (
            AssetId::parse("pf_copy").expect("ID"),
            "content/copy/box.prefab".into(),
        ),
    );
    mappings.assets.insert(
        "sc_kit".into(),
        (
            AssetId::parse("sc_copy").expect("ID"),
            "content/copy/main.scene".into(),
        ),
    );
    for (doc, old, new) in [
        ("pf_box", "e_lid", "e_newlid"),
        ("sc_kit", "e_instance", "e_newinstance"),
        ("sc_kit", "e_lid", "e_unrelated"),
    ] {
        mappings
            .records
            .insert((doc.into(), old.into()), RecordId::parse(new).expect("ID"));
    }
    let mut instance = scene.record("e_instance").expect("instance").clone();
    mappings.rewrite_record(&scene, &mut instance);
    assert_eq!(instance["prefab"], "@content/copy/box.prefab#pf_copy");
    assert_eq!(
        instance["overrides"]["e_newlid/Transform.position"],
        json!([0, 2, 0])
    );
    assert_eq!(instance["removed"], json!(["e_newlid"]));
    let mut external = scene.record("e_external").expect("external").clone();
    mappings.rewrite_record(&scene, &mut external);
    assert_eq!(external["prefab"], "@content/shared/light.prefab#pf_light");
    assert_eq!(external["overrides"]["e_lid/Light.intensity"], 5);
}
#[test]
fn safe_delete_reports_outside_owners_without_false_id_prefix_matches() {
    let mut workspace = Workspace::default();
    workspace.insert(
        "content/box.prefab",
        document("prefab", "pf_box", json!({"e_root":{"name":"Box"}})),
    );
    workspace.insert("content/main.scene",document("scene","sc_main",json!({"e_use":{"prefab":"@content/box.prefab#pf_box"},"e_other":{"prefab":"@content/other.prefab#pf_boxlong"}})));
    let ids = BTreeSet::from(["pf_box".to_owned()]);
    assert_eq!(
        blockers(&workspace, "content/box.prefab", &ids),
        ["content/main.scene (e_use.prefab)"]
    );
}
#[test]
fn modern_storage_paths_never_leak_into_short_references() {
    let prefab = document("prefab", "pf_box", json!({"e_root":{"name":"Box"}}));
    let source = SourceDoc {
        path: "game/kit/box.prefab".into(),
        reference_path: "kit/box.prefab".into(),
        document: prefab,
    };
    let mappings = Mappings::new("kit", Some("warehouse"), &[source]);
    let mut record=json!({"prefab":"@kit/box.prefab#pf_box","main":"kit/box.prefab","source":"<image src=\"@kit/box.prefab#pf_box\"/>"}).as_object().expect("record").clone();
    mappings.rewrite_record(&document("scene", "sc_main", json!({})), &mut record);
    assert_eq!(record["prefab"], "@warehouse/box.prefab#pf_box");
    assert_eq!(record["main"], "kit/box.prefab");
    assert_eq!(
        record["source"],
        "<image src=\"@warehouse/box.prefab#pf_box\"/>"
    );
    assert!(
        !serde_json::to_string(&record)
            .expect("json")
            .contains("game/")
    );
}
#[test]
fn delete_also_checks_reference_bearing_object_keys() {
    let mut workspace = Workspace::default();
    workspace.insert(
        "game/main.scene",
        document(
            "scene",
            "sc_main",
            json!({"e_use":{"lookup":{"@kit/box.prefab#pf_box":"value"}}}),
        ),
    );
    assert_eq!(
        blockers(
            &workspace,
            "kit/box.prefab",
            &BTreeSet::from(["pf_box".to_owned()])
        ),
        ["game/main.scene (e_use.lookup)"]
    );
}
#[test]
fn moving_props_never_renames_an_entity_or_custom_text_named_props() {
    let prefab = document("prefab", "pf_box", json!({"e_root":{"name":"props"}}));
    let mappings = Mappings::new(
        "props",
        Some("warehouse"),
        &[source("props/box.prefab", prefab)],
    );
    let scene = document("scene", "sc_main", json!({}));
    let mut record=json!({"name":"props","description":"keep props/box.prefab as the design label","custom":{"props":"props/box.prefab"},"prefab":"@props/box.prefab#pf_box"}).as_object().expect("record").clone();
    mappings.rewrite_record(&scene, &mut record);
    assert_eq!(record["name"], "props");
    assert_eq!(
        record["description"],
        "keep props/box.prefab as the design label"
    );
    assert_eq!(record["custom"]["props"], "props/box.prefab");
    assert_eq!(record["prefab"], "@warehouse/box.prefab#pf_box");
    let mut workspace = Workspace::default();
    workspace.insert(
        "game/main.scene",
        document(
            "scene",
            "sc_main",
            json!({"e_label":{"name":"props","description":"props/box.prefab"}}),
        ),
    );
    assert!(
        blockers(&workspace, "props", &BTreeSet::from(["pf_box".to_owned()])).is_empty(),
        "ordinary labels do not become delete blockers"
    );
}
#[test]
fn canonical_json_owners_rewrite_only_their_declared_path_fields() {
    let mut preset = json!({"name":"props","icon":"props/icon.ico","scriptProject":"props/player.csproj","content":"game","templates":[{"source":"props/artifact.zip"}]});
    assert!(rewrite_json_paths(
        JsonOwner::ExportPreset,
        "props",
        "warehouse",
        &mut preset
    ));
    assert_eq!(preset["name"], "props");
    assert_eq!(preset["icon"], "warehouse/icon.ico");
    assert_eq!(preset["scriptProject"], "warehouse/player.csproj");
    assert_eq!(
        preset["templates"][0]["source"], "props/artifact.zip",
        "artifact pins stay project-relative"
    );
    let mut resources = json!({"fonts":["props/body.ttf"],"images":{"props":"props/icon.png"},"click":"props/click.ogg","sounds":[{"component":"props","clip":"@props/click.ogg#as_click"}]});
    assert!(rewrite_json_paths(
        JsonOwner::UiResources,
        "props",
        "warehouse",
        &mut resources
    ));
    assert_eq!(resources["fonts"][0], "warehouse/body.ttf");
    assert_eq!(resources["images"]["props"], "warehouse/icon.png");
    assert_eq!(resources["click"], "warehouse/click.ogg");
    assert_eq!(resources["sounds"][0]["component"], "props");
    assert_eq!(
        resources["sounds"][0]["clip"],
        "@warehouse/click.ogg#as_click"
    );
    assert!(json_uses_path(
        JsonOwner::ExportPreset,
        "warehouse/icon.ico",
        &BTreeSet::new(),
        &preset
    ));
    assert!(
        !json_uses_path(JsonOwner::ExportPreset, "props", &BTreeSet::new(), &preset),
        "names/artifact pins are not authored references"
    );
    let mut marker = json!({"name":"props","main":"props/start.scene"});
    rewrite_json_paths(JsonOwner::Marker, "props", "warehouse", &mut marker);
    assert_eq!(
        marker,
        json!({"name":"props","main":"warehouse/start.scene"})
    );
    let mut policy = json!({"main":"props/start.scene","look":{"default":"props/day.look","name":"props"},"custom":"props/day.look"});
    rewrite_json_paths(JsonOwner::Policy, "props", "warehouse", &mut policy);
    assert_eq!(policy["main"], "warehouse/start.scene");
    assert_eq!(policy["look"]["default"], "warehouse/day.look");
    assert_eq!(policy["look"]["name"], "props");
    assert_eq!(policy["custom"], "props/day.look");
    assert!(
        json_uses_path(
            JsonOwner::UiResources,
            "props/icon.png",
            &BTreeSet::from(["as_icon".to_owned()]),
            &json!({"images":{"brand":"@#as_icon"}})
        ),
        "id-only consumers block deletion too"
    );
}
#[test]
fn unknown_authored_json_tracks_explicit_references_without_guessing_bare_paths() {
    let mut value = json!({"name":"props","custom":"props/icon.png","icon":"props/icon.png","reference":"@props/icon.png#as_icon"});
    assert!(json_uses_path(
        JsonOwner::Generic,
        "props",
        &BTreeSet::new(),
        &value
    ));
    rewrite_json_paths(JsonOwner::Generic, "props", "warehouse", &mut value);
    assert_eq!(
        value,
        json!({"name":"props","custom":"props/icon.png","icon":"props/icon.png","reference":"@warehouse/icon.png#as_icon"})
    );
    assert!(!json_uses_path(
        JsonOwner::Generic,
        "props",
        &BTreeSet::new(),
        &value
    ));
}

#[test]
fn selected_headerless_ui_move_pins_identity_through_a_typed_source_edit() {
    let text =
        "<?xml version=\"1.0\"?>\n<ui id=\"hud\"><text id=\"title\" text=\"Ready\" /></ui>\n";
    let ui = document("ui", "ui_legacy", json!({"us_source":{"source":text}}));
    let mappings = Mappings::new(
        "ui/game.ui",
        Some("features/hud.ui"),
        &[source("ui/game.ui", ui.clone())],
    );
    let mut workspace = Workspace::default();
    workspace.insert("ui/game.ui", ui.clone());
    let edits = fixups(&workspace, &mappings);
    let [
        Edit::Set {
            doc,
            id,
            path,
            value: Some(Value::String(pinned)),
        },
    ] = edits.as_slice()
    else {
        panic!("expected one typed UI source edit: {edits:?}");
    };
    assert_eq!(doc, &ui.id);
    assert_eq!(id.as_str(), "us_source");
    assert_eq!(path, &FieldPath::field("source"));
    assert_eq!(
        pinned,
        "<?xml version=\"1.0\"?>\n<!-- ara:asset ui_legacy -->\n\n<ui id=\"hud\"><text id=\"title\" text=\"Ready\" /></ui>\n"
    );
    assert_eq!(
        workspace
            .doc("ui_legacy")
            .expect("unchanged original")
            .record("us_source")
            .expect("record")["source"],
        text,
        "the op retains original bytes for undo"
    );
    let mut again = json!({"source":pinned})
        .as_object()
        .expect("record")
        .clone();
    mappings.rewrite_record(&ui, &mut again);
    assert_eq!(
        again["source"],
        pinned.as_str(),
        "identity pinning is idempotent"
    );
}

#[test]
fn duplicate_and_import_pin_the_selected_ui_mapped_identity() {
    let ui = document(
        "ui",
        "ui_original",
        json!({"us_source":{"source":"<ui id=\"hud\" />\n"}}),
    );
    for target in [Some("copy/game.ui"), None] {
        let mut mappings = Mappings::new("ui/game.ui", target, &[source("ui/game.ui", ui.clone())]);
        mappings.assets.insert(
            "ui_original".into(),
            (
                AssetId::parse("ui_mapped").expect("mapped ID"),
                "copy/game.ui".into(),
            ),
        );
        let mut record = ui.record("us_source").expect("source").clone();
        mappings.rewrite_record(&ui, &mut record);
        assert_eq!(
            record["source"],
            "<!-- ara:asset ui_mapped -->\n<ui id=\"hud\" />\n"
        );
    }
}

#[test]
fn explicit_ui_identity_is_remapped_without_adding_another_comment() {
    let ui = document(
        "ui",
        "ui_original",
        json!({"us_source":{"source":"<!-- ara:asset ui_original -->\n<ui id=\"ui_original\" />\n"}}),
    );
    let mut mappings = Mappings::new(
        "ui/game.ui",
        Some("copy/game.ui"),
        &[source("ui/game.ui", ui.clone())],
    );
    mappings.assets.insert(
        "ui_original".into(),
        (
            AssetId::parse("ui_copy").expect("copy ID"),
            "copy/game.ui".into(),
        ),
    );
    let mut record = ui.record("us_source").expect("source").clone();
    mappings.rewrite_record(&ui, &mut record);
    assert_eq!(
        record["source"],
        "<!-- ara:asset ui_copy -->\n<ui id=\"ui_copy\" />\n"
    );
    assert_eq!(
        record["source"]
            .as_str()
            .expect("XML")
            .matches("<!-- ara:asset ")
            .count(),
        1
    );
}

#[test]
fn inbound_headerless_ui_is_repaired_without_pinning_its_unmoved_identity() {
    let prefab = document("prefab", "pf_box", json!({"e_root":{}}));
    let mappings = Mappings::new(
        "props",
        Some("warehouse"),
        &[source("props/box.prefab", prefab)],
    );
    let ui = document("ui", "ui_inbound", json!({}));
    let mut record =
        json!({"source":"<ui id=\"hud\"><image src=\"@props/box.prefab#pf_box\" /></ui>\n"})
            .as_object()
            .expect("record")
            .clone();
    mappings.rewrite_record(&ui, &mut record);
    assert_eq!(
        record["source"],
        "<ui id=\"hud\"><image src=\"@warehouse/box.prefab#pf_box\" /></ui>\n"
    );
}
