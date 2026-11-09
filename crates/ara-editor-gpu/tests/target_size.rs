//! The UI target is exactly the window's drawable when the host pins it: an integer
//! logical size times a fractional scale misses it by a pixel (2560 px at 1.5 is
//! 1706.67 logical, which renders 1707 x 1.5 = 2561 px), and presenting would then
//! rescale the whole UI with a nearest blit, dropping a pixel column from the text.
use ara_editor_gpu::{NativeUi, open_presentation};
use ara_ui_render::{atlas::TextMode, gpu::Renderer as UiRenderer};

#[test]
fn a_pinned_target_matches_the_drawable_at_fractional_scales() -> ara_ui::Result<()> {
    let mut text = ara_ui_text::TextSystem::bundled()?;
    let drawable = [2560u32, 1369];
    let dpi = 1.5;
    let mut ui = ara_ui::Ui::editor(
        "<ui id='target'><text id='label' text='Crisp' /></ui>",
        ara_ui::theme::EDITOR_CSS,
        drawable.map(|v| v as f32 / dpi),
    )?;
    ui.frame(&mut text)?;
    let device = open_presentation(&[])?;
    // SAFETY: `device` outlives this adapter; the test thread owns every queue.
    let mut gpu = unsafe { NativeUi::new(&device)? };
    gpu.set_dpi(dpi)?;
    let mut draw = UiRenderer::default();
    let logical = ui.size.map(|v| v.round() as u32);
    draw.render(
        &mut gpu,
        &ui.display,
        &text.assets,
        logical,
        TextMode::Raster,
    )?;
    assert_eq!(
        gpu.output()?.1,
        [2561, 1370],
        "unpinned: rounded logical size x dpi"
    );
    gpu.pin_target_size(drawable);
    draw.render(
        &mut gpu,
        &ui.display,
        &text.assets,
        logical,
        TextMode::Raster,
    )?;
    assert_eq!(gpu.output()?.1, drawable);
    Ok(())
}
