use anyhow::{Context, Result, ensure};
use std::{fs::File, io::BufReader};

/// Verify that disabling ray queries changes visibility and only brightens pixels.
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let on_path = args.next().context("Expected shadowed PNG")?;
    let off_path = args.next().context("Expected unshadowed PNG")?;
    let output = args.next().context("Expected output JSON path")?;
    let (on_info, on) = read(&on_path)?;
    let (off_info, off) = read(&off_path)?;
    ensure!(
        on_info.width == off_info.width && on_info.height == off_info.height,
        "Dimensions differ"
    );
    let mut changed = 0_u32;
    let mut brighter = 0_u32;
    let mut sum = 0_u64;
    let mut max_delta = 0_u32;
    for (shadowed, unshadowed) in on.chunks_exact(4).zip(off.chunks_exact(4)) {
        let mut delta = 0_u32;
        for channel in 0..3 {
            delta += u32::from(shadowed[channel].abs_diff(unshadowed[channel]));
        }
        changed += u32::from(delta != 0);
        brighter += u32::from((0..3).any(|c| shadowed[c] > unshadowed[c]));
        sum += u64::from(delta);
        max_delta = max_delta.max(delta);
    }
    ensure!(
        changed > 1000 && brighter == 0,
        "Expected visible shadows: {changed} changed, {brighter} brighter pixels"
    );
    let evidence = serde_json::json!({
        "shadowed": on_path, "unshadowed": off_path,
        "pixels": on_info.width * on_info.height,
        "changed_pixels": changed, "shadowed_pixels_brighter_than_control": brighter,
        "mean_channel_absolute_difference": sum as f64 / f64::from(on_info.width * on_info.height * 3),
        "maximum_pixel_rgb_absolute_sum": max_delta,
    });
    std::fs::write(output, serde_json::to_vec_pretty(&evidence)?)?;
    println!("Verified {changed} shadow pixels; no shadowed pixel brighter than control");
    Ok(())
}

fn read(path: &str) -> Result<(png::OutputInfo, Vec<u8>)> {
    let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    let mut reader = decoder.read_info()?;
    let mut bytes = vec![
        0;
        reader
            .output_buffer_size()
            .context("PNG exceeds addressable memory")?
    ];
    let info = reader.next_frame(&mut bytes)?;
    ensure!(
        info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight,
        "Expected RGBA8 PNG"
    );
    bytes.truncate(info.buffer_size());
    Ok((info, bytes))
}
