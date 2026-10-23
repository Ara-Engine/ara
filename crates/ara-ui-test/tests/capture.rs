use ara_ui_stories::{Locale, StoryView};
use ara_ui_test::{CaptureEngine, CaptureReport, Result, Tolerance, compare, report_schema};
use tiny_skia::Pixmap;

fn view() -> StoryView {
    StoryView {
        width: 200,
        height: 120,
        dpi: 192,
        locale: "en".into(),
        ..StoryView::default()
    }
}
fn locale() -> Locale {
    Locale {
        id: "en".into(),
        ..Locale::default()
    }
}

#[test]
fn dpi_and_report_contract_match_the_rendered_pixels() -> Result<()> {
    let mut engine = CaptureEngine::bundled()?;
    let capture = engine.document(
        "<ui id='root'><button id='go' text='Go' action='game.go'/></ui>",
        "",
        &view(),
        &locale(),
    )?;
    let png = Pixmap::decode_png(&capture.png)?;
    assert_eq!([png.width(), png.height()], [400, 240]);
    assert_eq!(capture.report.pixels, [400, 240]);
    assert_eq!(capture.report.nodes[0].bounds.w, 200.0);
    let button = capture
        .report
        .nodes
        .iter()
        .find(|n| n.id == "go")
        .ok_or("button")?;
    assert!(
        button
            .provenance
            .iter()
            .any(|p| p["selector"] == "button" && p.get("valueBytes").is_some())
    );
    assert!(
        capture
            .report
            .nodes
            .iter()
            .any(|n| !n.provenance.is_empty())
    );
    assert!(
        capture
            .report
            .batches
            .iter()
            .any(|b| b.cause == "pipeline change")
    );
    assert!(
        capture
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "ARA7106" && d.threshold == Some(44.0))
    );
    let encoded = serde_json::to_value(&capture.report)?;
    let decoded: CaptureReport = serde_json::from_value(encoded)?;
    assert_eq!(decoded.version, 1);
    let mut db = ara_schema::SchemaDb::new();
    ara_ui_test::register(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    assert_eq!(report_schema()["properties"]["version"]["type"], "integer");
    let schema_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/capture-ui.schema.json");
    assert_eq!(
        std::fs::read_to_string(schema_path)?,
        ara_json::to_canonical_string(&report_schema())
    );
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("evidence/contract");
    let paths = capture.write(&directory, "capture")?;
    let parsed: CaptureReport = serde_json::from_str(&std::fs::read_to_string(&paths[1])?)?;
    assert_eq!(parsed.nodes.len(), capture.report.nodes.len());
    assert!(capture.write(&directory, "../invalid").is_err());
    Ok(())
}

#[test]
fn diagnostics_cover_overflow_text_fallback_and_exact_or_uncertain_contrast() -> Result<()> {
    let mut engine = CaptureEngine::bundled()?;
    let mut view = view();
    view.dpi = 96;
    let oversized = engine.document(
        "<ui id='root' style='width:300px;height:180px;'/>",
        "",
        &view,
        &locale(),
    )?;
    assert!(
        oversized
            .report
            .diagnostics
            .iter()
            .any(|d| d.code == "ARA7100" && d.node == "root")
    );
    let capture = engine.document("<ui id='root'><text id='text' text='A very long string of visible text' style='width:20px;height:12px;overflow:hidden;'/><button id='overflow' text='Go' style='width:240px;'/><text id='arabic' text='سلام'/><text id='missing' text='\u{10ffff}'/></ui>",
        "ui{padding:0px;gap:0px;background:#000000;} text {color:#ffffff;}", &view, &locale())?;
    let ds = &capture.report.diagnostics;
    for code in ["ARA7100", "ARA7101", "ARA7102", "ARA7103", "ARA7106"] {
        assert!(ds.iter().any(|d| d.code == code), "missing {code}");
    }
    assert!(
        ds.iter()
            .any(|d| d.code == "ARA7104" && d.measured.is_some_and(|v| (v - 21.0).abs() < 0.01))
    );
    engine.image(
        "@image.png#im_a",
        ara_ui_render::gpu::Image {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        },
    )?;
    let image = engine.document("<ui id='root'><image id='image' src='@image.png#im_a' style='position:absolute;width:200px;height:120px;'/><text id='text' text='Overlay'/></ui>", "", &view, &locale())?;
    assert!(
        image
            .report
            .diagnostics
            .iter()
            .any(|d| d.node == "text" && d.uncertain && d.measured.is_none())
    );
    Ok(())
}

#[test]
fn perceptual_tolerance_alpha_dimension_and_failure_artifacts() -> Result<()> {
    let mut a = Pixmap::new(16, 16).ok_or("bitmap")?;
    a.fill(tiny_skia::Color::WHITE);
    let identical = a.encode_png()?;
    assert!(compare(&identical, &identical, Tolerance::default())?.pass);
    let mut b = a.clone();
    b.data_mut()[0..3].copy_from_slice(&[254, 254, 254]);
    assert!(compare(&identical, &b.encode_png()?, Tolerance::default())?.pass);
    b.fill(tiny_skia::Color::BLACK);
    assert!(!compare(&identical, &b.encode_png()?, Tolerance::default())?.pass);
    assert!(
        !compare(
            &identical,
            &Pixmap::new(15, 16).ok_or("bitmap")?.encode_png()?,
            Tolerance::default()
        )?
        .pass
    );
    let transparent = Pixmap::new(16, 16).ok_or("bitmap")?.encode_png()?;
    assert!(!compare(&identical, &transparent, Tolerance::default())?.pass);
    assert!(
        compare(
            &identical,
            &identical,
            Tolerance {
                max_ssim_loss: f64::NAN,
                ..Tolerance::default()
            }
        )
        .is_err()
    );
    let mut engine = CaptureEngine::bundled()?;
    let capture = engine.document(
        "<ui id='root'><button id='go' text='Go'/></ui>",
        "",
        &view(),
        &locale(),
    )?;
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("evidence/failure-proof");
    let error = capture
        .compare(&identical, Tolerance::default(), &directory, "intentional")
        .expect_err("mismatch")
        .to_string();
    assert!(error.contains("#go") && error.contains("Diff image"));
    assert!(directory.join("intentional-diff.png").is_file());
    Ok(())
}
