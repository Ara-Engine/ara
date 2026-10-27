//! Program icon regeneration. Committed design outputs are compared by decoded pixels,
//! since the Rust and original Node PNG encoders use different compression.
use super::*;

const KINDS: [&str; 3] = ["engine", "game", "hub"];
const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// Render the original small drawing through 32 px, then the full drawing.
fn render_icon(dir: &Path, kind: &str, size: u32) -> Result<RgbaImage> {
    let suffix = if size <= 32 { "-small" } else { "" };
    render(&dir.join(format!("ara-{kind}{suffix}.svg")), size)
}

/// Check committed outputs without writes, or explicitly regenerate them.
pub(super) fn export(brand: &Path, check: bool) -> Result<()> {
    for kind in KINDS {
        let dir = brand.join("icons").join(kind);
        let mut frames = Vec::new();
        for size in PNG_SIZES {
            let rendered = render_icon(&dir, kind, size)?;
            let path = dir.join(format!("ara-{kind}-{size}.png"));
            if check {
                let committed = image::open(&path)?.into_rgba8();
                ensure!(committed == rendered, "{} differs from SVG", path.display());
            } else {
                rendered.save(path)?;
            }
            if SIZES.contains(&size) {
                frames.push(rendered);
            }
        }
        let path = dir.join(format!("ara-{kind}.ico"));
        if check {
            ensure!(
                read_ico(&path)? == frames,
                "{} differs from SVG",
                path.display()
            );
        } else {
            write_ico(&path, &frames)?;
        }
    }
    println!(
        "{} engine, game and Hub icons",
        if check { "checked" } else { "exported" }
    );
    Ok(())
}

/// Decode every PNG entry, including 256 encoded as a zero byte in ICONDIR.
fn read_ico(path: &Path) -> Result<Vec<RgbaImage>> {
    let bytes = std::fs::read(path)?;
    ensure!(bytes.get(..4) == Some(&[0, 0, 1, 0]), "invalid ICONDIR");
    let count = u16::from_le_bytes(bytes.get(4..6).context("ICO count")?.try_into()?) as usize;
    let mut images = Vec::new();
    for index in 0..count {
        let start = 6 + 16 * index;
        let entry = bytes.get(start..start + 16).context("ICO entry")?;
        let len = u32::from_le_bytes(entry[8..12].try_into()?) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into()?) as usize;
        let data = bytes
            .get(offset..offset.checked_add(len).context("ICO overflow")?)
            .context("ICO payload")?;
        let rgba = image::load_from_memory_with_format(data, image::ImageFormat::Png)?.into_rgba8();
        let extent = |n: u8| if n == 0 { 256 } else { u32::from(n) };
        ensure!(
            rgba.dimensions() == (extent(entry[0]), extent(entry[1])),
            "ICO size mismatch"
        );
        images.push(rgba);
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brand() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brand")
    }

    #[test]
    fn committed_outputs_match_rust_regeneration() -> Result<()> {
        export(&brand(), true)
    }

    #[test]
    fn program_icos_decode_all_required_sizes() -> Result<()> {
        for kind in KINDS {
            let frames = read_ico(&brand().join(format!("icons/{kind}/ara-{kind}.ico")))?;
            let dimensions: Vec<_> = frames.iter().map(RgbaImage::dimensions).collect();
            for size in [16, 32, 48, 256] {
                assert!(dimensions.contains(&(size, size)), "{kind}: missing {size}");
            }
        }
        Ok(())
    }

    #[test]
    fn programs_embed_their_own_resource() -> Result<()> {
        let root = brand().join("../..");
        for (program, kind) in [
            ("ara-editor", "engine"),
            ("ara-setup", "engine"),
            ("ara-hub", "hub"),
            ("ara-player", "game"),
        ] {
            let script = std::fs::read_to_string(root.join(format!("crates/{program}/build.rs")))?
                .split_whitespace()
                .collect::<String>();
            let resource = format!("../../assets/brand/icons/{kind}/ara-{kind}.rc");
            assert!(
                script.contains(&format!("embed_resource::compile(\"{resource}\"")),
                "{program}"
            );
            assert!(
                script.contains(&format!("rerun-if-changed={resource}")),
                "{program}"
            );
            assert!(
                script.contains(&format!(
                    "rerun-if-changed=../../assets/brand/icons/{kind}/ara-{kind}.ico"
                )),
                "{program}"
            );
            let rc = std::fs::read_to_string(brand().join(format!("icons/{kind}/ara-{kind}.rc")))?;
            assert!(
                rc.contains(&format!("1 ICON \"ara-{kind}.ico\"")),
                "{program}"
            );
        }
        Ok(())
    }
}
