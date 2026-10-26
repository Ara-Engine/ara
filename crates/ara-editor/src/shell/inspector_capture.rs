//! Opt-in offscreen captures of the production schema inspector on the native GPU.
#![allow(clippy::unwrap_used)]
use super::*;

#[test]
#[ignore = "offscreen Vulkan evidence; set ARA_INSPECTOR_CAPTURE_DIR under target"]
fn native_inspector_component_captures() {
    let output = std::env::var_os("ARA_INSPECTOR_CAPTURE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target/inspector-captures"));
    let root = output.join(format!("project-{}", std::process::id()));
    std::fs::create_dir_all(root.join("levels")).unwrap();
    std::fs::write(
        root.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"name":"Inspector capture","main":"levels/main.scene"}),
        ),
    )
    .unwrap();
    std::fs::write(root.join("levels/main.scene"), ara_json::to_canonical_string(&json!({"$schema":"ara://schema/scene/1","id":"sc_inspect01","records":{
        "e_light001":{"name":"Spot light","Transform":{},"Light":{"kind":"spot","intensity":1200,"color":[1,0.65,0.32,1]}},
        "e_mesh0001":{"name":"Salvage crate","Transform":{"position":[1.5,0.6,-2]},"MeshRenderer":{}},
        "e_post0001":{"name":"Post process","Transform":{},"PostProcess":{}}
    }}))).unwrap();
    let mut editor = Editor::open(&root).unwrap();
    editor
        .action("editor.preferences", json!({"reduceMotion":true}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [500., 1050.]).unwrap();
    shell.offscreen = true;
    shell
        .run_command(
            &mut editor,
            "window.maximize",
            &commands::Target::Panel("inspector".into()),
        )
        .unwrap();
    let device = ara_gpu::vulkan::VulkanDevice::new_raster(&[], output.join("gpu-cache")).unwrap();
    // SAFETY: this headless device outlives the UI adapter, and this thread owns its queue.
    let mut gpu = unsafe { crate::gpu::NativeUi::new(device.context()) }.unwrap();
    let mut draw = ara_ui_render::gpu::Renderer::default();
    icons::install(&mut draw, 1.).unwrap();
    let mut timings = Vec::new();
    for (name, entity, component) in [
        ("Light", "e_light001", "Light"),
        ("Transform", "e_mesh0001", "Transform"),
        ("Mesh", "e_mesh0001", "MeshRenderer"),
        ("PostProcess", "e_post0001", "PostProcess"),
    ] {
        editor
            .action("editor.select", json!({"entities":[entity]}))
            .unwrap();
        let start = std::time::Instant::now();
        shell.rebuild(&mut editor).unwrap();
        let rebuild_ms = start.elapsed().as_secs_f64() * 1000.;
        shell.reveal_field(component).unwrap();
        shell.settle_transitions();
        shell.frame().unwrap();
        draw.render(
            &mut gpu,
            &shell.ui.display,
            &shell.text.assets,
            [500, 1050],
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
    }
    std::fs::write(
        output.join("timings.json"),
        ara_json::to_canonical_string(&json!(timings)),
    )
    .unwrap();
    drop(gpu);
    device.assert_clean().unwrap();
}
