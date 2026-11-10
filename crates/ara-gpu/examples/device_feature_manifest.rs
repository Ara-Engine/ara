//! Explicit canonical preparation for device feature probes.
fn main() -> anyhow::Result<()> {
    use ara_gpu::{cooked::RenderState, descriptors::Format};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        root.join("tests/feature_shaders/hit.slang"),
        format!(
            "{}\n{}\n{}",
            include_str!("../tests/feature_shaders/hit_prefix.slang"),
            ara_gpu::hit_geometry::SHADER,
            include_str!("../tests/feature_shaders/hit_body.slang")
        ),
    )?;
    ara_gpu::cooked::write_json(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/feature_shaders/pipelines.json"),
        &serde_json::json!({
            "views": {"source":"views.slang", "entries":{"viewsMain":"compute"}, "state":RenderState::default()},
            "samplers": {"source":"samplers.slang", "entries":{"samplerMain":"compute"}, "state":RenderState::default()},
            "hit": {"source":"hit.slang", "entries":{"hitMain":"compute"}, "state":RenderState::default()},
            "color": {"source":"dual.slang", "entries":{"fullscreenVertex":"vertex", "colorOnly":"fragment"}, "state":RenderState {color_formats: vec![Format::Rgba8.vk().as_raw()], ..Default::default()}},
            "depth": {"source":"dual.slang", "entries":{"fullscreenVertex":"vertex", "depthOnly":"fragment"}, "state":RenderState {depth_format: Some(Format::Depth32Float.vk().as_raw()), depth_test:true, depth_write:true, ..Default::default()}},
            "dual": {"source":"dual.slang", "entries":{"fullscreenVertex":"vertex", "dualColor":"fragment"}, "state":RenderState {color_formats: vec![Format::Rgba8.vk().as_raw();2], ..Default::default()}}
        }),
    )
}
