//! Explicit offline fixture preparation; ordinary tests never rewrite sources.
fn main() -> anyhow::Result<()> {
    ara_gpu::cooked::write_json(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/pipelines.json"),
        &serde_json::json!({"sampled_heap": {
            "source": "sampled_heap.slang",
            "entries": {"sampleMain": "compute"},
            "state": ara_gpu::cooked::RenderState::default()
        }, "comparison": {
            "source": "comparison.slang",
            "entries": {"comparisonMain": "compute"},
            "state": ara_gpu::cooked::RenderState::default()
        }, "streams": {
            "source": "raster.slang",
            "entries": {"streams": "vertex", "color": "fragment"},
            "state": ara_gpu::cooked::RenderState {
                color_formats: vec![ara_gpu::descriptors::Format::Rgba8.vk().as_raw()],
                ..Default::default()
            }
        }, "fullscreen": {
            "source": "raster.slang",
            "entries": {"fullscreen": "vertex", "color": "fragment"},
            "state": ara_gpu::cooked::RenderState {
                color_formats: vec![ara_gpu::descriptors::Format::Rgba8.vk().as_raw()],
                ..Default::default()
            }
        }, "triangle_ranges": {
            "source": "triangle_ranges.slang",
            "entries": {"rangeProbe": "compute"},
            "state": ara_gpu::cooked::RenderState::default()
        }}),
    )
}
