//! Explicit offline fixture preparation; ordinary tests never rewrite sources.
fn main() -> anyhow::Result<()> {
    ara_gpu::cooked::write_json(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shaders/pipelines.json"),
        &serde_json::json!({"sampled_heap": {
            "source": "sampled_heap.slang",
            "entries": {"sampleMain": "compute"},
            "state": ara_gpu::cooked::RenderState::default()
        }}),
    )
}
