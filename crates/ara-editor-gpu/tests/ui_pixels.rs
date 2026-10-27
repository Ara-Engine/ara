//! The editor UI adapter draws what the CPU reference draws, on this platform's backend
//! (Metal on macOS, Vulkan elsewhere) and without a window: nested rounded clips,
//! premultiplied alpha, raster and MSDF text and an antialiased line. Captures go under
//! `target/tmp/ara-editor-gpu-ui-pixels`.
use ara_editor_gpu::{NativeUi, open_presentation};
use ara_ui_render::{CpuGpu, atlas::TextMode, gpu::Renderer as UiRenderer};

#[test]
fn native_ui_matches_the_cpu_reference() -> ara_ui::Result<()> {
    let output = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("ara-editor-gpu-ui-pixels");
    std::fs::create_dir_all(&output)?;
    let mut text = ara_ui_text::TextSystem::bundled()?;
    let mut ui = ara_ui::Ui::editor(
        "<ui id='comparison'><panel id='clip' style='width:220px; height:90px; overflow:hidden; border-radius:18px; background:var(--card)'><panel id='inner' style='width:180px; height:80px; overflow:hidden; border-radius:12px; background:var(--accent-surface)'><text id='raster-label' text='Ara raster / clipped' style='font-size:18px' /><button id='sample' text='Premultiplied alpha' /></panel></panel></ui>",
        ara_ui::theme::EDITOR_CSS,
        [300., 140.],
    )?;
    ui.frame(&mut text)?;
    ui.display.commands.push(ara_ui::paint::Draw::Line {
        node: 901,
        from: [12., 130.],
        to: [270., 105.],
        width: 2.,
        color: ara_ui::style::Color(76, 130, 230, 255),
    });
    let device = open_presentation(&[])?;
    for (mode, name) in [(TextMode::Raster, "raster"), (TextMode::Msdf, "msdf")] {
        let mut draw = UiRenderer::default();
        let mut cpu = CpuGpu::default();
        draw.render(&mut cpu, &ui.display, &text.assets, [300, 140], mode)?;
        draw.device_lost();
        // SAFETY: `device` outlives this adapter; the test thread owns every queue.
        let mut gpu = unsafe { NativeUi::new(&device)? };
        draw.render(&mut gpu, &ui.display, &text.assets, [300, 140], mode)?;
        let actual = gpu.pixels()?;
        let reference = image::RgbaImage::from_raw(300, 140, cpu.pixmap()?.data().to_vec())
            .ok_or("CPU reference extent")?;
        actual.save(output.join(format!("gpu-{name}.png")))?;
        reference.save(output.join(format!("cpu-{name}.png")))?;
        // The editor's GPU/CPU contract (`ara-editor --verify`): local SSIM loss <= 0.01.
        let comparison = ara_render::capture::compare(&reference, &actual, 0.01)?;
        comparison
            .diff
            .save(output.join(format!("gpu-cpu-{name}.diff.png")))?;
        eprintln!("{name}: GPU/CPU SSIM loss {:.5}", comparison.score);
        assert!(
            comparison.passed,
            "{name}: GPU/CPU SSIM loss {:.5} exceeds 0.01 (captures in {})",
            comparison.score,
            output.display()
        );
    }
    Ok(())
}
