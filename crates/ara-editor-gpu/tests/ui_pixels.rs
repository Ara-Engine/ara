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
        // The GPU/CPU contract: local SSIM loss <= 0.01.
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

/// Satin materials (D31) on the GPU against the CPU reference: a dock, a raised button,
/// a well and a glass menu (backdrop blur over stripes, layered shadows).
#[test]
fn satin_materials_and_glass_match_the_cpu_reference() -> ara_ui::Result<()> {
    let output = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("ara-editor-gpu-ui-pixels");
    std::fs::create_dir_all(&output)?;
    let mut text = ara_ui_text::TextSystem::bundled()?;
    let size = [320u32, 180];
    let stripes: String = (0..16)
        .map(|n| {
            format!(
                "<panel id='s{n}' class='{}' />",
                if n % 2 == 0 { "light" } else { "dark" }
            )
        })
        .collect();
    let css = "ui { width:100%; height:100%; background:var(--chrome); padding:0px; gap:0px; }
        .stripes { position:absolute; left:0px; top:0px; width:320px; height:180px; flex-direction:row; }
        .light { width:20px; height:180px; background:var(--foreground); }
        .dark { width:20px; height:180px; background:var(--accent); }
        .dock { position:absolute; left:12px; top:12px; width:150px; height:150px; background:var(--card); border-radius:10px; padding:12px; gap:10px;
          box-shadow: inset 0 1px 0 var(--dock-highlight), 0 0 0 1px var(--edge-shadow), 0 4px 14px var(--shadow); }
        .raised { height:26px; background:var(--raised); border-radius:6px;
          box-shadow: inset 0 1px 0 var(--edge-highlight), inset 0 -1px 0 var(--edge-lowlight), 0 0 0 1px var(--edge-shadow); }
        .well { height:26px; background:var(--well); border-radius:6px; box-shadow: inset 0 1px 0 var(--well-shade), 0 1px 0 var(--well-lip); }
        .menu { position:absolute; left:150px; top:40px; width:150px; height:110px; background:var(--menu-surface); border-radius:10px;
          backdrop-filter: blur(12px) saturate(1.2);
          box-shadow: inset 0 1px 0 var(--edge-highlight), inset 0 0 0 1px var(--float-ring), 0 0 0 1px var(--edge-shadow), 0 14px 34px var(--float-shadow); }";
    let mut ui = ara_ui::Ui::editor(
        &format!(
            "<ui id='satin'><panel id='stripes' class='stripes'>{stripes}</panel><panel id='dock' class='dock'><panel id='button' class='raised' /><panel id='field' class='well' /></panel><panel id='menu' class='menu' /></ui>"
        ),
        css,
        [size[0] as f32, size[1] as f32],
    )?;
    ui.frame(&mut text)?;
    let mut draw = UiRenderer::default();
    let mut cpu = CpuGpu::default();
    draw.render(&mut cpu, &ui.display, &text.assets, size, TextMode::Raster)?;
    draw.device_lost();
    let device = open_presentation(&[])?;
    // SAFETY: `device` outlives this adapter; the test thread owns every queue.
    let mut gpu = unsafe { NativeUi::new(&device)? };
    draw.render(&mut gpu, &ui.display, &text.assets, size, TextMode::Raster)?;
    let actual = gpu.pixels()?;
    let reference = image::RgbaImage::from_raw(size[0], size[1], cpu.pixmap()?.data().to_vec())
        .ok_or("CPU reference extent")?;
    actual.save(output.join("gpu-satin.png"))?;
    reference.save(output.join("cpu-satin.png"))?;
    let comparison = ara_render::capture::compare(&reference, &actual, 0.01)?;
    comparison
        .diff
        .save(output.join("gpu-cpu-satin.diff.png"))?;
    eprintln!("satin: GPU/CPU SSIM loss {:.5}", comparison.score);
    assert!(
        comparison.passed,
        "satin: GPU/CPU SSIM loss {:.5} exceeds 0.01 (captures in {})",
        comparison.score,
        output.display()
    );
    Ok(())
}
