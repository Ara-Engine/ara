//! Opt-in offscreen captures of the production schema inspector on the native GPU.
#![allow(clippy::unwrap_used)]
use super::*;

#[derive(Default)]
struct CaptureClipboard(String);
impl ara_ui_text::Clipboard for CaptureClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.to_owned();
        Ok(())
    }
}

#[test]
#[ignore = "offscreen Vulkan evidence; set ARA_INSPECTOR_CAPTURE_DIR under target"]
fn native_inspector_component_captures() {
    let output = std::env::var_os("ARA_INSPECTOR_CAPTURE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/inspector-captures"));
    let root = output.join(format!("project-{}", std::process::id()));
    std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
    std::fs::create_dir_all(root.join(".ara")).unwrap();
    std::fs::write(
        root.join(".ara/editor-layout.json"),
        ara_json::to_canonical_string(
            &json!({"Area":{"id":"max","tabs":["inspector"],"active":0}}),
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"name":"Inspector capture","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"levels/main.scene"}),
        ),
    )
    .unwrap();
    std::fs::write(root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"), ara_json::to_canonical_string(&json!({"$schema":"ara://schema/scene/1","id":"sc_inspect01","records":{
        "e_light001":{"name":"Spot light","Transform":{},"Light":{"kind":"spot","intensity":1200,"color":[1,0.65,0.32,1]}},
        "e_mesh0001":{"name":"Salvage crate","Transform":{"position":[1.5,0.6,-2]},"MeshRenderer":{}},
        "e_post0001":{"name":"Post process","Transform":{},"PostProcessVolume":{"overrides":[{"module":"toneMapping","toneMapping":"filmic"},{"module":"vignette","vignette":0.39},{"module":"chromaticAberration","chromaticAberration":0}]}},
        "e_listen01":{"name":"Listener","Transform":{"position":[-1,0,-1]},"AudioListener":{}}
    }}))).unwrap();
    let textures = root.join(ara_doc::AUTHORING_ROOT).join("textures");
    std::fs::create_dir_all(&textures).unwrap();
    image::RgbaImage::from_pixel(16, 16, image::Rgba([180, 120, 60, 255]))
        .save(textures.join("crate.png"))
        .unwrap();
    let mut editor = Editor::open(&root).unwrap();
    editor
        .apply(
            vec![json!({"cmd":"import.initialize","args":{"source":"textures/crate.png","id":"as_crate001","settings":{"maxDimension":2048}}})],
            "Import capture texture",
            None,
        )
        .unwrap();
    editor
        .action("editor.preferences", json!({"reduceMotion":true}))
        .unwrap();
    // Narrow docks (ARA_INSPECTOR_CAPTURE_WIDTH=360) show how rows compress.
    let width: u32 = std::env::var("ARA_INSPECTOR_CAPTURE_WIDTH")
        .ok()
        .and_then(|w| w.parse().ok())
        .unwrap_or(500);
    let mut shell = Shell::new(&mut editor, [width as f32, 1050.]).unwrap();
    shell.offscreen = true;
    shell
        .run_command(
            &mut editor,
            "window.maximize",
            &super::commands::Target::Panel("inspector".into()),
        )
        .unwrap();
    let device = ara_gpu::GpuDevice::new_raster(&[], output.join("gpu-cache")).unwrap();
    // SAFETY: this headless device outlives the UI adapter, and this thread owns its queue.
    let mut gpu = unsafe { crate::gpu::NativeUi::new(crate::gpu::ui_device(&device)) }.unwrap();
    let mut draw = ara_ui_render::gpu::Renderer::default();
    icons::install(&mut draw, 1.).unwrap();
    let mut timings = Vec::new();
    for (name, entity) in [
        ("Light", "e_light001"),
        ("Transform", "e_mesh0001"),
        ("Mesh", "e_mesh0001"),
        ("PostProcess", "e_post0001"),
        ("Listener", "e_listen01"),
    ] {
        editor
            .action("editor.select", json!({"entities":[entity]}))
            .unwrap();
        let start = std::time::Instant::now();
        shell.rebuild(&mut editor).unwrap();
        let rebuild_ms = start.elapsed().as_secs_f64() * 1000.;
        shell.input.blur(&mut shell.ui);
        shell.settle_transitions();
        shell.frame().unwrap();
        draw.render(
            &mut gpu,
            &shell.ui.display,
            &shell.text.assets,
            [width, 1050],
            ara_ui_render::atlas::TextMode::Raster,
        )
        .unwrap();
        let start = std::time::Instant::now();
        for _ in 0..32 {
            shell.frame().unwrap();
        }
        timings.push(json!({"component":name,"rebuildMs":rebuild_ms,"idleFrameUs":start.elapsed().as_secs_f64()*1e6/32.}));
        let pixels = gpu.pixels().unwrap();
        pixels
            .save(output.join(format!("{name}.full.png")))
            .unwrap();
        let body = shell.ui.nodes[shell.ui.find("body-inspector").unwrap()].rect;
        image::imageops::crop_imm(
            &pixels,
            body.x.max(0.) as u32,
            body.y.max(0.) as u32,
            body.w.min(pixels.width() as f32 - body.x).max(1.) as u32,
            body.h.min(pixels.height() as f32 - body.y).max(1.) as u32,
        )
        .to_image()
        .save(output.join(format!("{name}.png")))
        .unwrap();
        std::fs::write(
            output.join(format!("{name}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )
        .unwrap();
        if name == "Light" {
            let mut clipboard = CaptureClipboard::default();
            shell.click("swatch-Light-color", &mut clipboard).unwrap();
            shell.dispatch(&mut editor).unwrap();
            shell.settle_transitions();
            shell.frame().unwrap();
            draw.render(
                &mut gpu,
                &shell.ui.display,
                &shell.text.assets,
                [width, 1050],
                ara_ui_render::atlas::TextMode::Raster,
            )
            .unwrap();
            gpu.pixels()
                .unwrap()
                .save(output.join("Light-picker.png"))
                .unwrap();
            shell.click("entity-name", &mut clipboard).unwrap();
            shell.dispatch(&mut editor).unwrap();
        }
    }
    // The Help dialogs, filled with this device's real report.
    shell.device_info = serde_json::to_string_pretty(device.info()).unwrap();
    for name in [
        "dialog-system",
        "dialog-about",
        "dialog-agents",
        "dialog-agents-manual",
    ] {
        match name {
            "dialog-system" => shell.dialog = Some(Dialog::System),
            "dialog-about" => shell.dialog = Some(Dialog::about()),
            "dialog-agents" => {
                // Real, read-only detection on this machine; nothing is connected.
                shell.open_agents();
                let start = std::time::Instant::now();
                while !shell.agents_detected() && start.elapsed().as_secs() < 30 {
                    shell.poll_agents(&mut editor).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
            _ => shell.agent_action(
                ara_agents::Client::OpenCode,
                super::agents_connect::AgentAction::Manual,
            ),
        }
        shell.rebuild(&mut editor).unwrap();
        shell.settle_transitions();
        shell.frame().unwrap();
        draw.render(
            &mut gpu,
            &shell.ui.display,
            &shell.text.assets,
            [width, 1050],
            ara_ui_render::atlas::TextMode::Raster,
        )
        .unwrap();
        gpu.pixels()
            .unwrap()
            .save(output.join(format!("{name}.png")))
            .unwrap();
    }
    shell.dialog = None;
    // The Rendering Debug panel with one effect off (its reset button shows).
    editor.state.render_debug.shadows = false;
    editor
        .action("editor.layout.show", json!({"panel":"render-debug"}))
        .unwrap();
    editor.sync();
    shell
        .run_command(
            &mut editor,
            "window.maximize",
            &super::commands::Target::Panel("render-debug".into()),
        )
        .unwrap();
    editor.sync();
    shell.rebuild(&mut editor).unwrap();
    shell.settle_transitions();
    shell.frame().unwrap();
    draw.render(
        &mut gpu,
        &shell.ui.display,
        &shell.text.assets,
        [width, 1050],
        ara_ui_render::atlas::TextMode::Raster,
    )
    .unwrap();
    gpu.pixels()
        .unwrap()
        .save(output.join("panel-render-debug.png"))
        .unwrap();
    // Preferences › Viewport (VSync) and a texture's import inspector (no quality row).
    let mut capture = |shell: &mut Shell, editor: &mut Editor, panel: &str, name: &str| {
        editor.sync();
        shell
            .run_command(
                editor,
                "window.maximize",
                &super::commands::Target::Panel(panel.into()),
            )
            .unwrap();
        editor.sync();
        shell.rebuild(editor).unwrap();
        shell.settle_transitions();
        shell.frame().unwrap();
        draw.render(
            &mut gpu,
            &shell.ui.display,
            &shell.text.assets,
            [width, 1050],
            ara_ui_render::atlas::TextMode::Raster,
        )
        .unwrap();
        gpu.pixels().unwrap().save(output.join(name)).unwrap();
    };
    shell
        .run_command(&mut editor, "snap.settings", &super::commands::Target::None)
        .unwrap();
    editor
        .action("editor.search", json!({"panel":"settings","query":"sync"}))
        .unwrap();
    capture(&mut shell, &mut editor, "settings", "settings-viewport.png");
    editor
        .action("editor.layout.show", json!({"panel":"inspector"}))
        .unwrap();
    let scanning = std::time::Instant::now();
    while !shell.poll_assets_for_test(&mut editor) && scanning.elapsed().as_secs() < 20 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    editor
        .action(
            "editor.workspace",
            json!({"asset":format!("{}/textures/crate.png", ara_doc::AUTHORING_ROOT)}),
        )
        .unwrap();
    capture(&mut shell, &mut editor, "inspector", "import-texture.png");
    std::fs::write(
        output.join("timings.json"),
        ara_json::to_canonical_string(&json!(timings)),
    )
    .unwrap();
    std::fs::write(output.join("capture-fixture.json"),ara_json::to_canonical_string(&json!({"project":root,"size":[width,1050],"captures":{"Light":"e_light001","Transform":"e_mesh0001","Mesh":"e_mesh0001","PostProcess":"e_post0001"}}))).unwrap();
    drop(gpu);
    device.assert_clean().unwrap();
}
