//! Production Inspector/import rows contain native Selects in both densities.
use super::*;
use ara_ui::Ui;

const LONG_LABEL: &str =
    "Long enum value with many small words · عرض لون · More text for a thin field";

fn measure(ui: &Ui, control: &str, row: &str) -> Value {
    let control = &ui.nodes[ui.find(control).expect("mounted native Select")];
    let row = &ui.nodes[ui.find(row).expect("mounted property row")];
    let layout = ui.tree.layout(control.layout_id).unwrap();
    let text = control.text.as_ref().expect("Select caption");
    let width = layout.size.width
        - layout.padding.left
        - layout.padding.right
        - layout.border.left
        - layout.border.right;
    let height = layout.size.height
        - layout.padding.top
        - layout.padding.bottom
        - layout.border.top
        - layout.border.bottom;
    let inside = control.rect.y >= row.rect.y - 0.5
        && control.rect.y + control.rect.h <= row.rect.y + row.rect.h + 0.5;
    json!({"control":control.element.id,"row":row.element.id,"controlBox":control.rect,"rowBox":row.rect,"lines":text.layout.len(),"label":text.value,"inside":inside,"textFits":text.layout.width()<=width+0.5 && text.layout.height()<=height+0.5,"contentSize":[width,height],"textSize":[text.layout.width(),text.layout.height()]})
}

fn check_rows(
    shell: &mut Shell,
    editor: &Editor,
    control: &str,
    row: &str,
    numeric: &str,
    minimum: f32,
) -> Value {
    let css = shell.stylesheet(editor);
    let control_index = shell.shown_node(control).unwrap_or_else(|| {
        let dropdowns: Vec<_> = shell.ui.nodes.iter().filter(|node| node.element.tag == "dropdown").map(|node|node.element.id.as_str()).collect();
        panic!("Missing real Inspector/import enum control {control}; doc={}, selection={:?}, dropdowns={dropdowns:?}",editor.state.doc,editor.state.selection)
    });
    assert_eq!(shell.ui.nodes[control_index].element.tag, "dropdown");
    // Model a localized caption update through the real retained binding seam;
    // the valid stored enum and its typed callback remain unchanged.
    shell.ui.set_text(control_index, LONG_LABEL).unwrap();
    let fixed = format!("{css}\n.prop-row {{ height:{minimum}px; min-height:0px; }}");
    shell.ui.set_stylesheet(&fixed).unwrap();
    shell.frame().unwrap();
    let before = measure(&shell.ui, control, row);
    assert_eq!(before["label"], LONG_LABEL);
    assert!(
        before["lines"].as_u64().unwrap() > 1,
        "fixture must actually wrap: {before}"
    );
    assert!(
        before["inside"] == false || before["textFits"] == false,
        "legacy fixed-height row must reproduce overlap or clipped text: {before}"
    );
    shell.ui.set_stylesheet(&css).unwrap();
    shell.frame().unwrap();
    let after = measure(&shell.ui, control, row);
    assert_eq!(
        after["inside"], true,
        "Select must fit its real row: {after}"
    );
    assert_eq!(
        after["textFits"], true,
        "Select caption must fit its content box: {after}"
    );
    let number = &shell.ui.nodes[shell
        .shown_node(numeric)
        .expect("ordinary numeric property row")];
    assert!(
        (number.rect.h - minimum).abs() <= 0.5,
        "ordinary numeric rows retain {minimum}px density, got {:?}",
        number.rect
    );
    let numeric_box = number.rect;
    assert_eq!(
        shell.frame().unwrap(),
        ara_ui::Work::default(),
        "stable rows do no idle work"
    );
    json!({"legacyFixedHeight":before,"autoRow":after,"numericRow":numeric_box,"minimum":minimum})
}

#[test]
fn narrow_inspector_and_import_selects_fit_auto_rows_without_expanding_numeric_density() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open([800., 760.]);
    let mut measurements = Vec::new();
    for (density, minimum) in [("compact", 24.), ("comfortable", 28.)] {
        editor
            .action(
                "editor.settings.set",
                json!({"key":"appearance.density","value":density}),
            )
            .unwrap();
        editor
            .action("editor.workspace", json!({"asset":""}))
            .unwrap();
        editor
            .action("editor.select", json!({"entities":["e_quality1"]}))
            .unwrap();
        editor
            .action("editor.focus", json!({"panel":"inspector"}))
            .unwrap();
        editor
            .action(
                "editor.search",
                json!({"panel":"inspector","query":"Light"}),
            )
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        let revision = editor.host.store().revision();
        let inspector = check_rows(
            &mut shell,
            &editor,
            "enum-Light-kind",
            "prop-Light-kind",
            "prop-Light-intensity",
            minimum,
        );
        editor
            .action(
                "editor.workspace",
                json!({"asset":format!("{}/textures/quality.png",ara_doc::AUTHORING_ROOT)}),
            )
            .unwrap();
        editor
            .action("editor.focus", json!({"panel":"inspector"}))
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        let import = check_rows(
            &mut shell,
            &editor,
            "import-setting-usage-options",
            "import-setting-usage-row",
            "import-setting-maxDimension-row",
            minimum,
        );
        assert_eq!(
            editor.host.store().revision(),
            revision,
            "caption/density/layout changes never author content"
        );
        measurements.push(json!({"density":density,"inspector":inspector,"import":import}));
    }
    let output = crate::test_target().join("editor-quality/row-sizing-latest.json");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    std::fs::write(output,ara_json::to_canonical_string(&json!({"cases":measurements,"before":"Legacy fixed property-row height, applied as a host-reset fixture on the same mounted production controls","after":"Production auto row with density minimum"}))).unwrap();
}

#[test]
fn history_styles_resolve_against_both_canonical_editor_themes() {
    let project = Project::new();
    let (editor, mut shell) = project.open([1000., 760.]);
    let mut markup = String::from("<ui id='history-theme-contract'>");
    shell.history_panel(&editor, &mut markup);
    markup.push_str("</ui>");
    let css = shell.stylesheet(&editor);
    for theme in [
        ara_ui::theme::EditorTheme::Dark,
        ara_ui::theme::EditorTheme::Light,
    ] {
        let mut ui = Ui::with_tokens(&markup, &css, [900., 620.], theme.tokens().unwrap()).unwrap();
        ui.frame(&mut shell.text).unwrap();
        for id in [
            "history-selected-id",
            "history-detail-reason",
            "history-transactions",
        ] {
            let index = ui.find(id).expect("actual History branch mounted");
            assert!(
                ui.nodes[index].rect.w > 1. && ui.nodes[index].rect.h > 1.,
                "{theme:?}: {id}"
            );
        }
    }
}
