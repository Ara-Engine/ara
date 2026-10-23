mod support;
use ara_ui::{Result, theme::EditorTheme};
use ara_ui_render::atlas::TextMode;
fn main() -> Result<()> {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("evidence");
    std::fs::create_dir_all(&out)?;
    let mut text = support::text()?;
    for (name, mut ui) in [
        ("inspector", support::inspector()?),
        ("dock", support::dock()?),
        ("text-input", support::text_input()?),
        ("arabic-bidi", support::text_lab()?),
        ("pause", support::pause()?),
        ("world", support::world()?),
    ] {
        support::capture(
            name,
            &mut ui,
            &mut text,
            &out,
            if name == "world" {
                TextMode::Msdf
            } else {
                TextMode::Raster
            },
        )?;
    }
    let mut light = support::inspector()?;
    light.switch_theme(EditorTheme::Light)?;
    support::capture(
        "inspector-light",
        &mut light,
        &mut text,
        &out,
        TextMode::Raster,
    )?;
    let (mut ui, mut view, model) = support::tree()?;
    view.scroll(&mut ui, &model, 50_000.5 * 26.0, 468.0)?;
    support::capture(
        "virtual-tree-100k",
        &mut ui,
        &mut text,
        &out,
        TextMode::Raster,
    )?;
    let (mut ui, mut view, model) = support::table()?;
    view.scroll(&mut ui, &model, 1_200.5 * 26.0, 260.0)?;
    support::capture("virtual-table", &mut ui, &mut text, &out, TextMode::Raster)?;
    println!("Captures: {}", out.display());
    Ok(())
}
