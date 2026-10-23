//! Offline compression inventory for both benchmark clips.
#[path = "../tests/common/mod.rs"]
mod common;
use ara_anim::*;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let skeleton = common::skeleton(65);
    for multiplier in [1.0, 2.0] {
        let (clip, report) = CookedClip::cook(
            &common::clip(&skeleton, multiplier),
            &skeleton,
            CompressionSettings::default(),
        )?;
        println!(
            "clip={multiplier}; runtime_bytes={}; ratio={:.6}; {report:?}",
            clip.runtime_bytes(),
            report.source_bytes as f64 / report.cooked_bytes as f64
        );
    }
    Ok(())
}
