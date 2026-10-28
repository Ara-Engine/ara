use super::*;

#[test]
fn failed_window_restores_only_its_tabs_and_keeps_authoring_and_other_floats() {
    let root = crate::test_target().join("failed-window-recovery");
    std::fs::create_dir_all(&root).unwrap();
    let directory = root.join(format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let project = directory.as_path();
    std::fs::create_dir_all(project.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
    std::fs::write(project.join("ara.project.json"), ara_json::to_canonical_string(&json!({
        "layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Window recovery","main":"levels/main.scene"
    }))).unwrap();
    std::fs::write(
        project
            .join(ara_doc::AUTHORING_ROOT)
            .join("levels/main.scene"),
        ara_json::to_canonical_string(
            &json!({"$schema":"ara://schema/scene/1","id":"sc_windowproof","records":{}}),
        ),
    )
    .unwrap();
    let mut editor = Editor::open_config(
        project,
        Some(project.join("profile.json")),
        Default::default(),
    )
    .unwrap();
    let mut shell = Shell::new(&mut editor, [1200., 800.]).unwrap();
    for (panel, rect) in [
        ("inspector", [120., 100., 500., 600.]),
        ("scene", [180., 140., 700., 650.]),
        ("console", [240., 180., 500., 400.]),
    ] {
        editor
            .action(
                "editor.layout.move",
                json!({"panel":panel,"slot":"float","rect":rect}),
            )
            .unwrap();
    }
    let console = editor
        .state
        .floating
        .iter()
        .find(|float| panel_titles(&float.root) == "Console")
        .unwrap()
        .clone();
    let scene = editor
        .state
        .floating
        .iter()
        .find(|float| panel_titles(&float.root) == "Scene")
        .unwrap()
        .clone();
    let inspector = editor
        .state
        .floating
        .iter()
        .find(|float| panel_titles(&float.root) == "Inspector")
        .unwrap()
        .clone();
    let authoring = editor.host.store().revision();
    let error = std::io::Error::other("injected native creation failure");
    redock(&mut editor, &mut shell, &scene, &error).unwrap();
    assert!(
        editor
            .state
            .floating
            .iter()
            .all(|float| float.id != scene.id)
    );
    assert!(editor.state.floating.contains(&console));
    assert!(editor.state.floating.contains(&inspector));
    assert!(editor.state.dock().unwrap().contains("scene"));
    redock(&mut editor, &mut shell, &inspector, &error).unwrap();
    assert_eq!(editor.state.floating, [console]);
    assert_eq!(editor.host.store().revision(), authoring);
    editor.action("editor.focus", json!({"panel":"scene"})).unwrap();
    shell.rebuild(&mut editor).unwrap();
    assert!(shell.shown_node("viewport-image").is_some());
    assert!(
        shell
            .status_note
            .as_deref()
            .unwrap()
            .contains("injected native creation failure")
    );
    editor
        .action(
            "editor.search",
            json!({"panel":"hierarchy","query":"still responsive"}),
        )
        .unwrap();
    assert_eq!(editor.state.searches["hierarchy"], "still responsive");
}
