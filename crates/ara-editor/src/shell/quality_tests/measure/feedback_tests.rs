//! Shared capture geometry rejects clipped feedback, including otherwise valid roots.
use super::*;
use crate::shell::feedback_geometry;
use ara_ui::{Ui, input::InputEvent, paint::Draw, source::stable_id, theme::EditorTheme};

fn mounted(markup: &str, css: &str, theme: EditorTheme, size: [f32; 2]) -> Ui {
    let mut ui = Ui::with_tokens(markup, css, size, theme.tokens().unwrap()).unwrap();
    let mut text = ara_ui_text::TextSystem::bundled().unwrap();
    ui.frame(&mut text).unwrap();
    ui
}

const CSS: &str = "ui { width:100%; height:100%; } text { font-size:12px; flex-shrink:0; }";

#[test]
fn feedback_measure_rejects_long_fixed_width_chips_and_offset_descendants() {
    for theme in [EditorTheme::Dark, EditorTheme::Light] {
        for (dpi, size) in [(1., [1440., 900.]), (2., [720., 450.])] {
            let markup = "<ui id='root'><panel id='drop-label' style='position:absolute;left:24px;top:24px;width:140px;height:28px;flex-direction:row'><panel id='chip-row' style='width:100%;height:28px'><text id='chip-copy' text='Move all selected entities into a destination whose label is much longer than the former fixed width' /></panel></panel></ui>";
            let ui = mounted(markup, CSS, theme, size);
            let audit = feedback_geometry::audit(&ui);
            assert!(audit.violations.iter().any(|finding| finding.contains("chip-copy") && finding.contains("shaped text")), "{theme:?} at {dpi}x must reject the old fixed chip: {:?}", audit.violations);

            let mut ui = mounted(
                "<ui id='root'><feedback id='drop-label' style='position:absolute;left:24px;top:24px;width:240px;height:50px'><panel id='chip-row' style='width:220px;height:40px'><text id='chip-copy' text='Move selection' /></panel></feedback></ui>",
                CSS,
                theme,
                size,
            );
            let copy = ui.find("chip-copy").unwrap();
            ui.nodes[copy].paint_offset = [230., 0.];
            ui.nodes[copy].dirty.paint = true;
            ui.frame(&mut ara_ui_text::TextSystem::bundled().unwrap())
                .unwrap();
            let audit = feedback_geometry::audit(&ui);
            assert!(
                audit
                    .violations
                    .iter()
                    .any(|finding| finding.contains("chip-copy")
                        && finding.contains("ancestor chip-row")),
                "{theme:?} at {dpi}x must check intermediate containers, not just root size: {:?}",
                audit.violations
            );
        }
    }
}

#[test]
fn feedback_measure_uses_emitted_origins_and_rejects_vertical_banner_clipping() {
    for theme in [EditorTheme::Dark, EditorTheme::Light] {
        let mut ui = mounted(
            "<ui id='root'><panel id='import-result' class='notice asset-job' style='position:absolute;left:24px;top:24px;width:500px;height:80px;padding:8px'><text id='import-result-copy' text='Import complete' /></panel></ui>",
            CSS,
            theme,
            [720., 450.],
        );
        for draw in &mut ui.display.commands {
            if let Draw::GlyphRun { node, origin, .. } = draw
                && *node == stable_id("import-result-copy")
            {
                origin[1] += 70.;
            }
        }
        let audit = feedback_geometry::audit(&ui);
        assert!(
            audit
                .violations
                .iter()
                .any(|finding| finding.contains("import-result-copy")
                    && finding.contains("own content box")),
            "emitted glyph origins must be measured: {:?}",
            audit.violations
        );

        let ui = mounted(
            "<ui id='root'><panel id='import-result' class='notice asset-job' style='position:absolute;left:24px;top:24px;width:500px;height:24px'><panel id='result-row' style='height:14px;flex-shrink:0'><text id='import-result-copy' style='height:10px;white-space:normal' text='Import complete with a detailed inventory of created, replaced, skipped and renamed content. This result needs several visible lines.' /></panel></panel></ui>",
            CSS,
            theme,
            [720., 450.],
        );
        let audit = feedback_geometry::audit(&ui);
        assert!(
            audit
                .violations
                .iter()
                .any(|finding| finding.contains("import-result-copy")
                    && finding.contains("shaped text")),
            "vertical clipping in banners must fail: {:?}",
            audit.violations
        );
        assert!(
            !audit
                .violations
                .iter()
                .any(|finding| finding.contains("too wide")),
            "banners do not inherit popover width limits"
        );
    }
}

#[test]
fn feedback_measure_allows_wide_banners_and_intentional_scroll_content() {
    let ui = mounted(
        "<ui id='root'><panel id='import-result' class='notice asset-job' style='position:absolute;left:24px;top:24px;width:700px;height:100px;padding:8px'><text id='import-result-copy' text='Import complete' /><scroll id='details' style='height:44px;flex-shrink:0'><panel id='detail-content' style='height:400px;flex-shrink:0'><text id='detail-copy' text='Scroll through the complete inventory' /></panel></scroll></panel></ui>",
        CSS,
        EditorTheme::Dark,
        [900., 620.],
    );
    let audit = feedback_geometry::audit(&ui);
    assert!(
        audit
            .surfaces
            .iter()
            .any(|surface| surface["surface"] == "import-result" && surface["maxWidth"].is_null())
    );
    assert!(
        audit.violations.is_empty(),
        "intentional scroll viewports are not clipped feedback: {:?}",
        audit.violations
    );
}

#[test]
fn feedback_measure_rejects_a_first_banner_taller_than_its_job_rail() {
    let ui = mounted(
        "<ui id='root'><scroll id='rail' style='position:absolute;left:24px;top:24px;width:500px;height:35px'><panel id='import-result' class='notice asset-job' style='height:80px;flex-shrink:0'><text id='import-result-copy' text='Import complete' /></panel></scroll></ui>",
        CSS,
        EditorTheme::Dark,
        [720., 450.],
    );
    let audit = feedback_geometry::audit(&ui);
    assert!(
        audit
            .violations
            .iter()
            .any(|finding| finding.contains("first visible banner clips vertically")),
        "a first job title without its result is not complete feedback: {:?}",
        audit.violations
    );
}

fn hierarchy_point(shell: &Shell, entity: &str) -> [f32; 2] {
    let index = shell
        .bindings
        .iter()
        .find_map(|(key, binding)| {
            if matches!(binding, Binding::Select(id) if id == entity) {
                shell.ui.by_id(*key).filter(|index| shell.ui.shown(*index))
            } else {
                None
            }
        })
        .expect("mounted hierarchy row");
    let rect = shell.ui.nodes[index]
        .rect
        .intersection(shell.ui.nodes[index].clip);
    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]
}

#[test]
fn active_pointer_drag_feedback_is_measured_in_both_themes_at_one_and_two_times_dpi() {
    let project = Project::new();
    let (mut editor, mut shell) = project.open([1440., 900.]);
    editor.apply(vec![json!({"cmd":"entity.create","args":{"id":"e_feedbacktarget","name":"A destination with enough descriptive words to wrap the complete drag feedback message","components":{"Transform":{}}}})], "Create geometry destination", None).unwrap();
    for light in [false, true] {
        for (dpi, size) in [(1., [1440., 900.]), (2., [720., 450.])] {
            editor
                .action("editor.theme", json!({"light":light}))
                .unwrap();
            editor
                .action("editor.select", json!({"entities":["e_quality1"]}))
                .unwrap();
            shell.ui.resize(size).unwrap();
            show(&mut editor, &mut shell, "hierarchy");
            let mut clip = Clip::default();
            let start = hierarchy_point(&shell, "e_quality1");
            shell.pointer = start;
            shell
                .event(InputEvent::PointerDown(start), &mut clip)
                .unwrap();
            shell.begin_drag();
            let destination = hierarchy_point(&shell, "e_feedbacktarget");
            shell.pointer = destination;
            shell
                .event(InputEvent::PointerMove(destination), &mut clip)
                .unwrap();
            shell.content_drag_over(&editor, 0.).unwrap();
            shell.frame().unwrap();
            assert!(
                shell.content_dragging(),
                "real pointer movement crosses the drag threshold"
            );
            let audit = feedback_geometry::audit(&shell.ui);
            assert!(
                audit
                    .surfaces
                    .iter()
                    .any(|surface| surface["surface"] == "drop-label"
                        && surface["kind"] == "feedback"),
                "active feedback must actually be measured"
            );
            assert!(
                audit.violations.is_empty(),
                "light={light}, dpi={dpi}: {:?}",
                audit.violations
            );
            shell.event(InputEvent::Cancel, &mut clip).unwrap();
        }
    }
}

#[test]
#[ignore = "requires a matching ara-import-worker; native OS-drop captures exercise the same audit"]
fn real_completed_import_inventory_uses_the_same_capture_geometry_gate() {
    let project = Project::new();
    let destination = "Descriptive imported content destination for the complete result banner";
    std::fs::create_dir_all(project.0.join(ara_doc::AUTHORING_ROOT).join(destination)).unwrap();
    let external = project.0.join("geometry-external.bin");
    std::fs::write(&external, b"geometry import fixture").unwrap();
    let external = std::fs::canonicalize(external).unwrap();
    let (mut editor, mut shell) = project.open([1440., 900.]);
    shell
        .queue_asset_import(asset_import::Request {
            sources: vec![external],
            destination: destination.into(),
            conflicts: Vec::new(),
        })
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let result = loop {
        editor.tick();
        shell.poll_asset_imports(&mut editor).unwrap();
        let report = shell.asset_import_report();
        assert!(
            !report["jobs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|job| job["record"]["status"] == "failed"),
            "typed import failed: {report}"
        );
        let result = report["jobs"]
            .as_array()
            .and_then(|jobs| {
                jobs.iter()
                    .find(|job| job["preflight"] == false && job["record"]["status"] == "done")
            })
            .cloned();
        if let Some(result) = result {
            break result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "typed import did not complete: {report}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(
        result["record"]["result"]["txn"].is_string(),
        "the measured result must belong to an actual attributed transaction: {result}"
    );
    assert_eq!(
        asset_import::completed_output(&result["record"])["created"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        project
            .0
            .join(ara_doc::AUTHORING_ROOT)
            .join(destination)
            .join("geometry-external.bin")
            .is_file()
    );
    for light in [false, true] {
        for (dpi, size) in [(1., [1440., 900.]), (2., [720., 450.])] {
            editor
                .action("editor.theme", json!({"light":light}))
                .unwrap();
            shell.ui.resize(size).unwrap();
            show(&mut editor, &mut shell, "assets");
            let audit = feedback_geometry::audit(&shell.ui);
            assert!(
                audit.surfaces.iter().any(|surface| surface["surface"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("asset-import-"))
                    && surface["kind"] == "banner"),
                "the real terminal job banner must actually be measured"
            );
            assert!(
                audit.violations.is_empty(),
                "light={light}, dpi={dpi}: {:?}",
                audit.violations
            );
        }
    }
}
