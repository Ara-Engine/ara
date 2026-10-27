//! Mounted hierarchy projections, explicit reveal and isolated preference persistence.
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
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let root = target.join("hierarchy-filters").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        let mut records: serde_json::Map<String, Value> = (0..1_000)
            .map(|index| {
                (
                    format!("e_a{index:04}"),
                    json!({"name":format!("Background {index}"),"Transform":{}}),
                )
            })
            .collect();
        for (id, record) in [
            ("e_hroot", json!({"name":"World context","Transform":{}})),
            (
                "e_ibranch",
                json!({"name":"Lighting group","parent":"e_hroot","Transform":{}}),
            ),
            (
                "e_jlight",
                json!({"name":"Key light","parent":"e_ibranch","Transform":{},"Light":{}}),
            ),
            (
                "e_kother",
                json!({"name":"O'Leary mesh","parent":"e_hroot","Transform":{},"MeshRenderer":{}}),
            ),
            (
                "e_larabic",
                json!({"name":"مصباح جانبي","parent":"e_ibranch","Transform":{},"Light":{}}),
            ),
        ] {
            records.insert(id.into(), record);
        }
        // Invalid raw IDs are rejected by the document loader; fail the fixture
        // itself instead of accidentally exercising a scene of background rows.
        for id in records.keys() {
            ara_doc::RecordId::parse(id).expect("valid hierarchy fixture record ID");
        }
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Hierarchy filters","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_hierfilter","records":records}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
    fn open(&self) -> (Editor, Shell) {
        let mut editor = Editor::open(&self.0).unwrap();
        assert_fixture(&editor);
        let shell = Shell::new(&mut editor, [1100., 700.]).unwrap();
        (editor, shell)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn ids(shell: &Shell) -> Vec<&str> {
    shell
        .hierarchy
        .items
        .iter()
        .filter_map(|(key, _)| shell.hierarchy_entities.get(key).map(String::as_str))
        .collect()
}
fn records(editor: &Editor) -> Value {
    json!(
        editor
            .host
            .store()
            .workspace()
            .doc(&editor.state.doc)
            .unwrap()
            .records()
    )
}
fn assert_fixture(editor: &Editor) {
    let document = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .expect("fixture scene");
    assert_eq!(
        document.records().len(),
        1_005,
        "all authored fixture records must load"
    );
    for id in ["e_hroot", "e_ibranch", "e_jlight", "e_kother", "e_larabic"] {
        assert!(
            document.records().contains_key(id),
            "missing fixture entity {id}"
        );
    }
}

#[test]
fn compound_queries_keep_real_ancestor_depth_and_stable_focus_without_changing_collapse_preferences()
 {
    let project = Project::new();
    let (mut editor, mut shell) = project.open();
    for entity in ["e_hroot", "e_ibranch"] {
        editor
            .action("editor.collapse", json!({"entity":entity,"enabled":true}))
            .unwrap();
    }
    editor
        .action("editor.select", json!({"entities":["e_jlight"]}))
        .unwrap();
    let collapsed = editor.state.collapsed.clone();
    let revision = editor.host.store().revision();
    for (query, leaf) in [
        ("Type:Light name:\"key light\"", "e_jlight"),
        ("id:E_JLIGHT -t:Camera", "e_jlight"),
        ("name:\"مصباح جانبي\" type:Light", "e_larabic"),
    ] {
        editor
            .action("editor.search", json!({"panel":"hierarchy","query":query}))
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert_eq!(ids(&shell), ["e_hroot", "e_ibranch", leaf], "{query}");
        for (depth, id) in ["e_hroot", "e_ibranch", leaf].into_iter().enumerate() {
            let style = &shell.hierarchy_style[&crate::world::render_id(id)];
            assert_eq!(style.depth, depth);
            assert!(!style.collapsed);
        }
        assert_eq!(editor.state.collapsed, collapsed);
        assert_eq!(editor.state.selection, ["e_jlight"]);
    }
    editor
        .action(
            "editor.search",
            json!({"panel":"hierarchy","query":"O'Leary"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert_eq!(ids(&shell), ["e_hroot", "e_kother"]);
    editor
        .action(
            "editor.search",
            json!({"panel":"hierarchy","query":"id:e_jlight"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let key = crate::world::render_id("e_jlight");
    let focused = shell.hierarchy_view.as_ref().unwrap().identity(key);
    shell.input.focus(&mut shell.ui, focused).unwrap();
    editor.apply(vec![json!({"cmd":"property.set","args":{"entity":"e_jlight","path":"name","value":"Background rename"}})],"Rename without navigating",None).unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert_eq!(shell.input.focused(), Some(focused));
    assert_eq!(editor.state.selection, ["e_jlight"]);
    assert_eq!(editor.state.collapsed, collapsed);
    assert_eq!(editor.host.store().revision(), revision + 1);
}

#[test]
fn explicit_filtered_reveal_unhides_ancestors_and_materializes_the_same_key_in_docked_or_floating_hierarchy()
 {
    for floating in [false, true] {
        let project = Project::new();
        let (mut editor, mut shell) = project.open();
        if floating {
            editor
                .action(
                    "editor.layout.move",
                    json!({"panel":"hierarchy","slot":"float","rect":[40,40,360,500]}),
                )
                .unwrap();
        }
        for entity in ["e_hroot", "e_ibranch", "e_jlight"] {
            editor
                .action("editor.collapse", json!({"entity":entity,"enabled":true}))
                .unwrap();
            editor
                .action("editor.visibility", json!({"entity":entity,"enabled":true}))
                .unwrap();
        }
        editor
            .action("editor.lock", json!({"entity":"e_jlight","enabled":true}))
            .unwrap();
        editor
            .action("editor.select", json!({"entities":["e_jlight"]}))
            .unwrap();
        editor
            .action(
                "editor.search",
                json!({"panel":"hierarchy","query":"t:Camera"}),
            )
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        assert!(shell.hierarchy.items.is_empty());
        let before = records(&editor);
        let revision = editor.host.store().revision();
        let camera = editor.state.camera;
        shell
            .reveal_hierarchy_entities(&mut editor, &Target::None)
            .unwrap();
        assert!(shell.poll_hierarchy_filters(&mut editor).unwrap());
        assert!(shell.hierarchy_filters.reveal.is_none());
        assert_eq!(shell.current(), None);
        if let Some(host) = shell.hierarchy_surface.clone() {
            assert!(shell.enter(&host));
        }
        shell.frame().unwrap();
        let key = crate::world::render_id("e_jlight");
        let identity = shell.hierarchy_view.as_ref().unwrap().identity(key);
        assert_eq!(shell.input.focused(), Some(identity));
        let node = shell.ui.by_id(identity).unwrap();
        let row = &shell.ui.nodes[node];
        assert!(shell.ui.shown(node));
        assert!(row.rect.intersection(row.clip).h >= row.rect.h - 0.01);
        assert!(
            shell.hierarchy_offset > 0.,
            "deep stable key must materialize outside the initial pool"
        );
        assert_eq!(editor.state.selection, ["e_jlight"]);
        assert_eq!(editor.state.camera, camera);
        assert!(editor.state.locked.contains("e_jlight"));
        for entity in ["e_hroot", "e_ibranch", "e_jlight"] {
            assert!(!editor.state.hidden.contains(entity));
            assert!(!editor.state.collapsed.contains(entity));
        }
        assert_eq!(
            editor.state.searches.get("hierarchy").map(String::as_str),
            Some("")
        );
        assert_eq!(records(&editor), before);
        assert_eq!(editor.host.store().revision(), revision);
    }
}

#[test]
fn named_filter_prompt_merges_latest_preferences_selects_natively_and_persists_only_to_its_isolated_path()
 {
    let project = Project::new();
    let profile = project.0.join("test-profile/editor-settings.json");
    let mut editor = Editor::open_with(&project.0, Some(profile.clone())).unwrap();
    assert_fixture(&editor);
    let mut shell = Shell::new(&mut editor, [1100., 700.]).unwrap();
    let mut clipboard = crate::native::SessionClipboard::new(true);
    let before = records(&editor);
    let revision = editor.host.store().revision();
    editor
        .action(
            "editor.search",
            json!({"panel":"hierarchy","query":"t:Light"}),
        )
        .unwrap();
    shell
        .save_hierarchy_filter(&mut editor, &Target::None)
        .unwrap();
    // A concurrent settings write must survive the later human prompt result.
    editor
        .action(
            "editor.settings.set",
            json!({"key":"hierarchy.filters","value":{"Other search":"id:e_kother"}}),
        )
        .unwrap();
    let focused = shell.input.focused().expect("prompt field focus");
    assert!(matches!(
        shell.ui.widgets.get(&focused),
        Some(ara_ui::widgets::Widget::Input(_))
    ));
    assert!(shell.native_text("Lights مصابيح", &mut clipboard).unwrap());
    assert!(
        matches!(shell.ui.widgets.get(&focused),Some(ara_ui::widgets::Widget::Input(input)) if input.value()=="Lights مصابيح")
    );
    // Match Controller::event(TextInput): dispatch the native text action before
    // the next physical Enter, so the dialog's retained prompt value is current.
    shell.dispatch(&mut editor).unwrap();
    shell.key(&mut editor, 40, true, &mut clipboard).unwrap();
    assert!(shell.poll_hierarchy_filters(&mut editor).unwrap());
    assert_eq!(
        editor.state.settings["hierarchy.filters"]["Other search"],
        "id:e_kother"
    );
    assert_eq!(
        editor.state.settings["hierarchy.filters"]["Lights مصابيح"],
        "t:Light"
    );
    editor
        .action(
            "editor.search",
            json!({"panel":"hierarchy","query":"nothing"}),
        )
        .unwrap();
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    let control = shell.shown_node("hierarchy-saved-filter").unwrap();
    assert_eq!(shell.ui.nodes[control].element.tag, "dropdown");
    shell
        .input
        .focus(
            &mut shell.ui,
            ara_ui::source::stable_id("hierarchy-saved-filter"),
        )
        .unwrap();
    shell.key(&mut editor, 40, true, &mut clipboard).unwrap();
    shell.key(&mut editor, 77, true, &mut clipboard).unwrap();
    shell.key(&mut editor, 40, true, &mut clipboard).unwrap();
    assert_eq!(editor.state.searches["hierarchy"], "id:e_kother");
    shell
        .use_hierarchy_filter(&mut editor, &Target::Text("Other search".into()), true)
        .unwrap();
    assert!(
        editor.state.settings["hierarchy.filters"]
            .get("Other search")
            .is_none()
    );
    assert_eq!(
        editor.state.searches["hierarchy"], "id:e_kother",
        "deleting a preset keeps its active query"
    );
    assert_eq!(records(&editor), before);
    assert_eq!(editor.host.store().revision(), revision);
    drop(shell);
    drop(editor);
    let reopened = Editor::open_with(&project.0, Some(profile)).unwrap();
    assert_eq!(
        reopened.state.settings["hierarchy.filters"]["Lights مصابيح"],
        "t:Light"
    );
    assert!(
        reopened.state.settings["hierarchy.filters"]
            .get("Other search")
            .is_none()
    );
}
