//! Real authority tests: cross-document atomicity and latest independent overrides.
#![allow(clippy::unwrap_used)]
use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("import-batch")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT)).unwrap();
        std::fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Import batch","main":"main.scene"}))).unwrap();
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_importbatch","records":{}}),
            ),
        )
        .unwrap();
        // The import lifecycle validates existing source media before creating sidecars.
        let authored = root.join(ara_doc::AUTHORING_ROOT);
        for name in ["a.png", "b.png"] {
            image::RgbaImage::from_pixel(2, 2, image::Rgba([128, 128, 255, 255]))
                .save(authored.join(name))
                .unwrap();
        }
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../ara-player/demo/content/audio/drums.wav"),
            authored.join("other.wav"),
        )
        .unwrap();
        Self(root)
    }
    fn open(&self) -> Editor {
        let mut editor =
            Editor::open_with(&self.0, Some(self.0.join(".ara/test-user.json"))).unwrap();
        editor.apply(vec![
            json!({"cmd":"import.initialize","args":{"source":"a.png","id":"as_batcha","settings":{"maxDimension":512,"mipmaps":true}}}),
            json!({"cmd":"import.initialize","args":{"source":"b.png","id":"as_batchb","settings":{"maxDimension":2048,"mipmaps":false}}}),
            json!({"cmd":"import.initialize","args":{"source":"other.wav","id":"as_batchc","settings":{"opusBitrate":128000}}}),
        ],"Create typed import batch fixture",None).unwrap();
        editor
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0
                .starts_with(std::path::absolute(crate::test_target()).unwrap())
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn records(editor: &Editor) -> Value {
    let workspace = editor.host.store().workspace();
    json!([
        workspace.doc("as_batcha").unwrap().records(),
        workspace.doc("as_batchb").unwrap().records(),
        workspace.doc("as_batchc").unwrap().records()
    ])
}
fn setting(doc: &str, field: &str, value: Value) -> Change {
    Change::Setting {
        doc: doc.into(),
        field: field.into(),
        value,
    }
}

#[test]
fn independent_mixed_sources_commit_one_transaction_and_one_undo_redo() {
    let project = Project::new();
    let mut editor = project.open();
    let before = records(&editor);
    let revision = editor.host.store().revision();
    commands::apply(
        &mut editor,
        vec![
            setting("as_batcha", "flipNormalY", json!(true)),
            setting("as_batchb", "flipNormalY", json!(true)),
        ],
        "Batch flip normals",
    )
    .unwrap();
    let after = records(&editor);
    assert_eq!(editor.host.store().revision(), revision + 1);
    assert_eq!(after[0]["i_settings"]["settings"]["maxDimension"], 512);
    assert_eq!(after[1]["i_settings"]["settings"]["maxDimension"], 2048);
    assert_eq!(after[0]["i_settings"]["settings"]["mipmaps"], true);
    assert_eq!(after[1]["i_settings"]["settings"]["mipmaps"], false);
    assert_eq!(after[2], before[2], "incompatible audio type untouched");
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(records(&editor), before);
    editor.action("history.redo", json!({})).unwrap();
    assert_eq!(records(&editor), after);
}

#[test]
fn latest_other_field_change_is_preserved_and_invalid_member_rolls_back_every_source() {
    let project = Project::new();
    let mut editor = project.open();
    editor.apply(vec![json!({"cmd":"import.configure","args":{"source":"b.png","settings":{"maxDimension":4096,"mipmaps":false}}})],
        "Concurrent independent import setting",None).unwrap();
    commands::apply(
        &mut editor,
        vec![
            setting("as_batcha", "flipNormalY", json!(true)),
            setting("as_batchb", "flipNormalY", json!(true)),
        ],
        "Batch after independent change",
    )
    .unwrap();
    assert_eq!(
        records(&editor)[1]["i_settings"]["settings"]["maxDimension"],
        4096
    );
    let before = records(&editor);
    let revision = editor.host.store().revision();
    assert!(
        commands::apply(
            &mut editor,
            vec![
                setting("as_batcha", "mipmaps", json!(false)),
                setting("as_batchb", "maxDimension", json!(0))
            ],
            "Reject complete batch"
        )
        .is_err()
    );
    assert_eq!(records(&editor), before);
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn deleted_sidecar_rejects_batch_and_preset_inheritance_is_one_atomic_edit() {
    let project = Project::new();
    let mut editor = project.open();
    let before = records(&editor);
    commands::apply(
        &mut editor,
        vec![
            Change::Preset {
                doc: "as_batcha".into(),
                preset: Some(ara_asset::CookPreset::Fast),
            },
            Change::Preset {
                doc: "as_batchb".into(),
                preset: Some(ara_asset::CookPreset::Fast),
            },
        ],
        "Batch quality",
    )
    .unwrap();
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(records(&editor), before);
    let revision = editor.host.store().revision();
    assert!(
        commands::apply(
            &mut editor,
            vec![
                setting("as_batcha", "mipmaps", json!(false)),
                setting("as_missing", "mipmaps", json!(false))
            ],
            "Reject deleted reference"
        )
        .is_err()
    );
    assert_eq!(records(&editor), before);
    assert_eq!(editor.host.store().revision(), revision);
}

struct Clip;
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
fn click(shell: &mut Shell, editor: &mut Editor, id: &str) {
    let node = &shell.ui.nodes[shell.shown_node(id).unwrap()];
    let rect = node.rect.intersection(node.clip);
    assert!(
        rect.w > 1. && rect.h >= node.rect.h * 0.75,
        "{id}: {:?}, clip {:?}",
        node.rect,
        node.clip
    );
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    shell
        .event(ara_ui::input::InputEvent::PointerDown(point), &mut Clip)
        .unwrap();
    shell
        .event(ara_ui::input::InputEvent::PointerUp(point), &mut Clip)
        .unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
}
#[test]
fn mounted_multi_selection_type_select_and_mixed_toggle_use_the_real_input_path() {
    let project = Project::new();
    let mut editor = project.open();
    let mut shell = Shell::new(&mut editor, [1200., 900.]).unwrap();
    shell.offscreen = true;
    let (browser, mut publication) = assets::scan_fixture::controlled(ara_doc::AUTHORING_ROOT);
    shell.browser = browser;
    let items = [("a.png", "png"), ("b.png", "png"), ("other.wav", "wav")]
        .into_iter()
        .map(|(logical, kind)| {
            let mut asset = assets::Asset::unindexed(
                format!("{}/{logical}", ara_doc::AUTHORING_ROOT),
                kind.into(),
            );
            asset.authored_path = Some(logical.into());
            asset
        })
        .collect::<Vec<_>>();
    let keys = items
        .iter()
        .map(|asset| ara_ui::source::stable_id(&asset.path))
        .collect::<Vec<_>>();
    let primary = items[0].path.clone();
    let generation = publication.start();
    publication.publish(generation, items).unwrap();
    assert!(shell.poll_assets());
    editor
        .action(
            "editor.workspace",
            json!({"asset":primary,"folder":ara_doc::AUTHORING_ROOT,"grid":false}),
        )
        .unwrap();
    shell.asset_selection.set_selected(keys);
    shell.asset_selection_path = editor.state.asset_path.clone();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    click(&mut shell, &mut editor, "import-batch-type");
    shell.key(&mut editor, 77, true, &mut Clip).unwrap();
    shell.key(&mut editor, 40, true, &mut Clip).unwrap();
    shell.frame().unwrap();
    assert_eq!(shell.import_batch.filter.as_deref(), Some("ara.texture"));
    assert!(shell.shown_node("import-batch-mipmaps-mixed").is_some());
    let before = records(&editor);
    let revision = editor.host.store().revision();
    click(&mut shell, &mut editor, "import-batch-mipmaps");
    let after = records(&editor);
    assert_eq!(editor.host.store().revision(), revision + 1);
    assert_eq!(after[0]["i_settings"]["settings"]["mipmaps"], true);
    assert_eq!(after[1]["i_settings"]["settings"]["mipmaps"], true);
    assert_eq!(after[0]["i_settings"]["settings"]["maxDimension"], 512);
    assert_eq!(after[1]["i_settings"]["settings"]["maxDimension"], 2048);
    assert_eq!(after[2], before[2]);
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(records(&editor), before);
}

#[test]
fn schema_driven_project_fields_update_one_type_and_reset_is_one_undo() {
    let project = Project::new();
    let mut editor = project.open();
    let mut shell = Shell::new(&mut editor, [1200., 900.]).unwrap();
    shell.offscreen = true;
    shell.import_defaults(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(shell.shown_node("import-defaults-inspector").is_some());
    shell
        .import_batch_value(
            &mut editor,
            "@default:ara.audio",
            "opusBitrate",
            json!(96000),
        )
        .unwrap();
    let policy = commands::policy(&editor).unwrap();
    assert_eq!(policy.defaults.audio.unwrap().opus_bitrate, 96000);
    assert!(policy.defaults.texture.is_none());
    let revision = editor.host.store().revision();
    shell
        .reset_import_defaults(
            &mut editor,
            &crate::shell::commands::Target::Text("audio".into()),
        )
        .unwrap();
    assert_eq!(editor.host.store().revision(), revision + 1);
    assert!(commands::policy(&editor).unwrap().defaults.audio.is_none());
    editor.action("history.undo", json!({})).unwrap();
    assert_eq!(
        commands::policy(&editor)
            .unwrap()
            .defaults
            .audio
            .unwrap()
            .opus_bitrate,
        96000
    );
}
