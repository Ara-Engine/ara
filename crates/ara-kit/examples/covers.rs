//! A sheet of generated covers for twelve ids in both schemes: `target/ara-kit/covers.png`.
fn main() -> anyhow::Result<()> {
    let ids = ["outpost", "night-market", "courier-run", "prototype-7", "showcase", "salvage", "skyline", "ember", "drift", "lumen", "harbor", "atlas"];
    let (w, h) = (320u32, 180u32);
    let mut sheet = image::RgbaImage::new(w * 6 + 70, h * 4 + 50);
    for (row, dark) in [true, false].into_iter().enumerate() {
        let look = ara_kit::look::Look::new(dark);
        for (i, id) in ids.iter().enumerate() {
            let img = ara_kit::art::cover(&look, ara_kit::art::seed(id), w, h);
            let (ox, oy) = ((i as u32 % 6) * (w + 10) + 10, (row as u32 * 2 + i as u32 / 6) * (h + 10) + 10);
            for (j, p) in img.pixels.chunks(4).enumerate() {
                sheet.put_pixel(ox + j as u32 % w, oy + j as u32 / w, image::Rgba([p[2], p[1], p[0], 255]));
            }
        }
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-kit/covers.png");
    sheet.save(&out)?;
    println!("{}", out.display());
    Ok(())
}
