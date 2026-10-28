//! Opt-in full-shell GPU captures; device construction never creates a window.
use super::*;

#[test]
#[ignore = "offscreen GPU quality evidence; set ARA_EDITOR_QUALITY_CAPTURE_DIR under target"]
fn native_full_editor_overlay_quality_captures() {
    let output = std::env::var_os("ARA_EDITOR_QUALITY_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/editor-quality/captures"));
    std::fs::create_dir_all(&output).unwrap();
    let project = Project::new();
    let device = ara_gpu::GpuDevice::new_raster(&[], output.join("gpu-cache")).unwrap();
    // SAFETY: this thread owns the device queue; device outlives its UI adapter.
    let mut gpu = unsafe { crate::gpu::NativeUi::new(crate::gpu::ui_device(&device)) }.unwrap();
    let mut draw = ara_ui_render::gpu::Renderer::default();
    let mut captures = Vec::new();
    let mut capture = |shell: &mut Shell, name: &str| {
        let dpi = if name.starts_with("2x-") { 2. } else { 1. };
        gpu.set_dpi(dpi).unwrap();
        draw.set_raster_scale(dpi).unwrap();
        icons::install(&mut draw, dpi).unwrap();
        let physical = shell.ui.size.map(|value| (value * dpi).round() as u32);
        draw.render(
            &mut gpu,
            &shell.ui.display,
            &shell.text.assets,
            physical,
            ara_ui_render::atlas::TextMode::Raster,
        )
        .unwrap();
        let name = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect::<String>();
        let path = output.join(format!("{name}.png"));
        gpu.pixels().unwrap().save(&path).unwrap();
        captures.push(json!({"file":path.file_name().unwrap().to_string_lossy(),"dpi":dpi,"logical":shell.ui.size,"physical":physical}));
    };
    let sweep = run_sweep(&project, &mut capture);
    sweep.write(&output);
    std::fs::write(
        output.join("capture-index.json"),
        ara_json::to_canonical_string(&json!({"captures":captures})),
    )
    .unwrap();
    assert!(
        sweep.violations.is_empty(),
        "{}",
        sweep
            .violations
            .iter()
            .take(80)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
