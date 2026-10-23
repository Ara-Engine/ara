mod common;
use anyhow::Result;
use ara_render::{Renderer, capture::ViewMode};
use std::path::PathBuf;
fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("crates/ara-render/captures"));
    let mut renderer = Renderer::new(common::scene()?, [640, 360], &[], ".sn/gpu")?;
    for mode in ViewMode::ALL {
        let mut request = common::request([640, 360]);
        request.mode = mode;
        request.marks = if matches!(mode, ViewMode::Lit) {
            Some(40)
        } else {
            None
        };
        let capture = renderer.capture(&request)?;
        capture.write(&output.join(format!("{}.png", mode.name())))?;
    }
    for timing in renderer.timings() {
        println!(
            "{} ({:?}): {:.3} ms",
            timing.pass, timing.queue, timing.milliseconds
        );
    }
    renderer.gpu().assert_clean()?;
    Ok(())
}
