#[path = "../examples/support/mod.rs"]
mod support;
use ara_ui::{Result, theme::EditorTheme};
use ara_ui_render::atlas::TextMode;
fn golden(
    name: &str,
    ui: &mut ara_ui::Ui,
    text: &mut ara_ui_text::TextSystem,
    mode: TextMode,
) -> Result<()> {
    let (renderer, cpu) = support::render(ui, text, mode)?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("goldens")
        .join(format!("{name}.png"));
    for node in &ui.nodes {
        if let Some(text) = &node.text {
            assert_eq!(
                ara_ui_text::inspect(text).missing_glyphs,
                0,
                "missing glyphs: {}",
                node.element.id
            );
        }
    }
    if name == "arabic-bidi" {
        assert!(
            renderer
                .batches
                .iter()
                .any(|b| b.pipeline == ara_ui_render::gpu::Pipeline::ColorGlyph)
        );
        let sample = |id: &str| {
            ui.nodes[ui.find(id).expect("lab sample exists")]
                .text
                .as_ref()
                .expect("lab sample has text")
        };
        let arabic = ara_ui_text::inspect(sample("arabic"));
        assert!(arabic.rtl_runs > 0 && arabic.lines >= 2);
        let mixed = ara_ui_text::inspect(sample("mixed"));
        assert!(mixed.rtl_runs > 0 && mixed.ltr_runs > 0);
        assert!(ara_ui_text::inspect(sample("cjk")).lines >= 2);
        let text = sample("combining");
        let clusters = ara_ui_text::inspect(text).clusters;
        for sequence in [
            "e\u{301}",
            "a\u{308}",
            "n\u{303}",
            "\u{1f469}\u{1f3fd}\u{200d}\u{1f680}",
            "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}",
            "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}",
        ] {
            let start = text.value.find(sequence).expect("coverage sequence exists");
            assert!(
                clusters
                    .iter()
                    .any(|c| c.bytes == (start..start + sequence.len())),
                "sequence was split: {sequence}"
            );
        }
    }
    if std::env::var_os("ARA_UI_UPDATE_GOLDENS").is_some() {
        cpu.save(&path)?;
    } else {
        let expected = tiny_skia::Pixmap::load_png(&path)?;
        let actual = cpu.pixmap()?;
        assert_eq!(actual.width(), expected.width());
        assert_eq!(actual.height(), expected.height());
        if actual.data() != expected.data() {
            let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("{name}.actual.png"));
            std::fs::create_dir_all(path.parent().expect("target output directory"))?;
            cpu.save(&path)?;
            panic!("golden mismatch: {}", path.display());
        }
    }
    Ok(())
}
#[test]
fn editor_game_multilingual_world_and_light_goldens() -> Result<()> {
    let mut text = support::text()?;
    for (name, mut ui) in [
        ("inspector", support::inspector()?),
        ("dock", support::dock()?),
        ("text-input", support::text_input()?),
        ("arabic-bidi", support::text_lab()?),
        ("pause", support::pause()?),
        ("world", support::world()?),
    ] {
        golden(
            name,
            &mut ui,
            &mut text,
            if name == "world" {
                TextMode::Msdf
            } else {
                TextMode::Raster
            },
        )?;
    }
    let mut light = support::inspector()?;
    light.switch_theme(EditorTheme::Light)?;
    golden("inspector-light", &mut light, &mut text, TextMode::Raster)?;
    let (mut ui, mut view, model) = support::tree()?;
    view.scroll(&mut ui, &model, 50_000.5 * 26.0, 468.0)?;
    golden("virtual-tree-100k", &mut ui, &mut text, TextMode::Raster)?;
    let (mut ui, mut view, model) = support::table()?;
    view.scroll(&mut ui, &model, 1_200.5 * 26.0, 260.0)?;
    golden("virtual-table", &mut ui, &mut text, TextMode::Raster)?;
    Ok(())
}
