//! Per-frame cost of a live window resize through the editor's UI path: relayout,
//! render, swapchain resize, blit and present, once per size step. Compares the old
//! swapchain handling (device idle, destroy, create) with `NativeUi::resize_present`
//! exact and with a live-resize reach (one oversized swapchain presented 1:1). A
//! hidden window, so nothing shows on the desktop; validation must stay clean.
//! `cargo test -p ara-editor-gpu --test resize_timing -- --ignored --nocapture`
#![cfg(not(target_os = "macos"))]
use ara_editor_gpu::{NativeUi, open_presentation};
use ara_platform::Window;
use ara_ui_render::{atlas::TextMode, gpu::Renderer as UiRenderer};
use std::time::Instant;

#[test]
#[ignore = "requires a Vulkan desktop; prints timings"]
fn live_resize_frame_cost() -> ara_ui::Result<()> {
    let window = Window::hidden("Ara resize timing", [1280, 800])?;
    let device = open_presentation(&window.extensions()?)?;
    let mut text = ara_ui_text::TextSystem::bundled()?;
    let rows: String = (0..40)
        .map(|n| format!("<text id='row{n}' text='Property row {n}: 12.500 m/s' />"))
        .collect();
    let mut ui = ara_ui::Ui::editor(
        &format!("<ui id='resize'><panel id='side' style='width:320px'>{rows}</panel></ui>"),
        ara_ui::theme::EDITOR_CSS,
        [1280., 800.],
    )?;
    // SAFETY: `device` outlives the adapter and presentation; this thread owns its queues.
    let mut gpu = unsafe { NativeUi::new(&device)? };
    // SAFETY: the window outlives the presentation, retired after idle below.
    let mut present = unsafe { gpu.create_present(&device, &window)? };
    let mut draw = UiRenderer::default();
    let names = [
        "idle + destroy + create",
        "resize_present (exact)",
        "resize_present (live reach)",
    ];
    for (mode, name) in names.into_iter().enumerate() {
        let mut swapchain = Vec::new();
        let mut frame = Vec::new();
        for i in 0..90u32 {
            let size = [1280 + (i % 45) * 6, 800 + (i % 30) * 4];
            window.set_size(size)?;
            let _ = window.poll()?;
            let physical = window.size()?;
            let at = Instant::now();
            // SAFETY: as above; the old presentation is idle before it is destroyed.
            unsafe {
                match mode {
                    0 => {
                        gpu.wait_idle(&device)?;
                        gpu.retire_present(&device, &mut present);
                        present = gpu.create_present(&device, &window)?;
                    }
                    1 => gpu.resize_present(&device, &mut present, &window, None)?,
                    _ => gpu.resize_present(&device, &mut present, &window, Some([1700, 1000]))?,
                }
            }
            swapchain.push(at.elapsed().as_secs_f64() * 1000.);
            ui.resize(physical.map(|v| v as f32))?;
            ui.frame(&mut text)?;
            gpu.pin_target_size(physical);
            draw.render(
                &mut gpu,
                &ui.display,
                &text.assets,
                ui.size.map(|v| v.round() as u32),
                TextMode::Raster,
            )?;
            // SAFETY: the UI submission completed; owners are live.
            unsafe { gpu.present(&device, &mut present)? };
            frame.push(at.elapsed().as_secs_f64() * 1000.);
        }
        let stats = |mut ms: Vec<f64>| {
            ms.sort_by(f64::total_cmp);
            let mean = ms.iter().sum::<f64>() / ms.len() as f64;
            format!(
                "mean {mean:.2}, p50 {:.2}, p95 {:.2}, max {:.2}",
                ms[ms.len() / 2],
                ms[ms.len() * 95 / 100],
                ms[ms.len() - 1]
            )
        };
        println!(
            "{name}: swapchain [{}] ms; whole frame [{}] ms",
            stats(swapchain),
            stats(frame)
        );
    }
    gpu.wait_idle(&device)?;
    // SAFETY: device idle; not used again.
    unsafe { gpu.retire_present(&device, &mut present) };
    device.assert_clean()?;
    Ok(())
}
