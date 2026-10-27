//! `ara-brand`: dev-only tool that exports the Ara logo set and compresses README media.
//!
//! - `cargo run -p ara-brand -- export` renders `assets/brand/*.svg` into the PNG
//!   ladder, `ara.ico` and `favicon.ico` (see [`export`]).
//! - `cargo run -p ara-brand -- render <svg> <png> --size N` previews one SVG.
//! - `cargo run -p ara-brand -- media <in> <out.jpg> --width 1920` resizes a
//!   capture and encodes it as JPEG for `docs/media/`.

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use image::{
    ImageEncoder, RgbaImage,
    codecs::{
        ico::{IcoEncoder, IcoFrame},
        jpeg::JpegEncoder,
    },
    imageops::FilterType,
};
use resvg::{tiny_skia, usvg};
use std::path::{Path, PathBuf};
mod icons;

/// PNG ladder exported for every brand symbol.
const PNG_SIZES: [u32; 9] = [16, 24, 32, 48, 64, 128, 256, 512, 1024];
/// Frames in the Windows app icon.
const ICO_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];
/// Frames in the browser favicon.
const FAVICON_SIZES: [u32; 3] = [16, 32, 48];
/// Sizes at or below this use the optically adjusted `*-small.svg` master when it exists.
const SMALL_MAX: u32 = 24;

#[derive(Parser)]
#[command(about = "Export the Ara logo set and compress README media")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Regenerate the three program icons, or check their committed pixels.
    Icons {
        /// Brand directory containing icons/{engine,game,hub} SVGs and outputs.
        #[arg(long, default_value = "assets/brand")]
        dir: PathBuf,
        /// Verify PNG and ICO pixels without modifying committed outputs.
        #[arg(long)]
        check: bool,
    },
    /// Render the PNG ladder, ara.ico and favicon.ico from the brand SVGs.
    Export {
        /// Brand directory holding the SVG masters.
        #[arg(long, default_value = "assets/brand")]
        dir: PathBuf,
    },
    /// Render one SVG to a square PNG.
    Render {
        svg: PathBuf,
        png: PathBuf,
        #[arg(long, default_value_t = 256)]
        size: u32,
        /// Nearest-neighbour upscale factor, for inspecting small-size pixels.
        #[arg(long, default_value_t = 1)]
        zoom: u32,
    },
    /// Resize an image to a target width and encode it as JPEG.
    Media {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 1920)]
        width: u32,
        #[arg(long, default_value_t = 84)]
        quality: u8,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Icons { dir, check } => icons::export(&dir, check),
        Command::Export { dir } => export(&dir),
        Command::Render {
            svg,
            png,
            size,
            zoom,
        } => {
            let image = render(&svg, size)?;
            let (w, h) = (image.width() * zoom.max(1), image.height() * zoom.max(1));
            image::imageops::resize(&image, w, h, FilterType::Nearest).save(&png)?;
            Ok(())
        }
        Command::Media {
            input,
            output,
            width,
            quality,
        } => media(&input, &output, width, quality),
    }
}

/// Render an SVG file into a straight-alpha RGBA image whose longer side is `size`
/// (square masters give `size`×`size`).
fn render(svg: &Path, size: u32) -> Result<RgbaImage> {
    let data = std::fs::read(svg).with_context(|| format!("read {}", svg.display()))?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())?;
    let extent = tree.size();
    let scale = size as f32 / extent.width().max(extent.height());
    let [width, height] = [extent.width(), extent.height()].map(|n| (n * scale).round() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).context("pixmap size")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    RgbaImage::from_raw(width, height, rgba).context("pixmap layout")
}

/// Render `name` at `size`, preferring `name-small.svg` for tiny sizes.
fn render_symbol(dir: &Path, name: &str, size: u32) -> Result<RgbaImage> {
    let small = dir.join(format!("{name}-small.svg"));
    let master = if size <= SMALL_MAX && small.exists() {
        small
    } else {
        dir.join(format!("{name}.svg"))
    };
    render(&master, size)
}

/// Encode frames as a PNG-compressed multi-size `.ico`.
fn write_ico(path: &Path, frames: &[RgbaImage]) -> Result<()> {
    let frames = frames
        .iter()
        .map(|f| {
            IcoFrame::as_png(
                f.as_raw(),
                f.width(),
                f.height(),
                image::ExtendedColorType::Rgba8,
            )
        })
        .collect::<image::ImageResult<Vec<_>>>()?;
    let file = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    IcoEncoder::new(std::io::BufWriter::new(file)).encode_images(&frames)?;
    Ok(())
}

/// Export every derived brand file. Symbols: `ara-icon` (app tile) and the
/// transparent `ara-mark`. Light and mono variants ship as SVG only.
fn export(dir: &Path) -> Result<()> {
    let png_dir = dir.join("png");
    std::fs::create_dir_all(&png_dir)?;
    for name in ["ara-icon", "ara-mark"] {
        for size in PNG_SIZES {
            render_symbol(dir, name, size)?.save(png_dir.join(format!("{name}-{size}.png")))?;
        }
    }
    let icon = |sizes: &[u32]| -> Result<Vec<_>> {
        sizes
            .iter()
            .map(|&s| render_symbol(dir, "ara-icon", s))
            .collect()
    };
    write_ico(&dir.join("ara.ico"), &icon(&ICO_SIZES)?)?;
    write_ico(&dir.join("favicon.ico"), &icon(&FAVICON_SIZES)?)?;
    icons::export(dir, false)?;
    println!(
        "exported {} PNGs, ara.ico, favicon.ico into {}",
        PNG_SIZES.len() * 2,
        dir.display()
    );
    Ok(())
}

/// Downscale (never upscale) to `width` with Lanczos3 and write a baseline JPEG.
fn media(input: &Path, output: &Path, width: u32, quality: u8) -> Result<()> {
    ensure!((1..=100).contains(&quality), "quality must be 1..=100");
    let image = image::open(input).with_context(|| format!("open {}", input.display()))?;
    let image = if image.width() > width {
        let height = (image.height() as u64 * width as u64 / image.width() as u64) as u32;
        image.resize_exact(width, height, FilterType::Lanczos3)
    } else {
        image
    };
    let rgb = image.to_rgb8();
    let file =
        std::fs::File::create(output).with_context(|| format!("create {}", output.display()))?;
    JpegEncoder::new_with_quality(std::io::BufWriter::new(file), quality).write_image(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    let bytes = std::fs::metadata(output)?.len();
    println!(
        "{} -> {} ({}x{}, {} KB)",
        input.display(),
        output.display(),
        rgb.width(),
        rgb.height(),
        bytes / 1024
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brand_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/brand")
    }

    #[test]
    fn small_icon_master_keeps_tile_and_mark_at_16px() {
        let icon = render_symbol(&brand_dir(), "ara-icon", 16).expect("render");
        assert_eq!(icon.dimensions(), (16, 16));
        assert!(
            icon.get_pixel(0, 0)[3] < 128,
            "rounded corner stays transparent"
        );
        let white = icon
            .pixels()
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200)
            .count();
        assert!(white > 12, "the white mark survives at 16 px ({white} px)");
    }

    #[test]
    fn ico_holds_every_frame() {
        let path = std::env::temp_dir().join(format!("ara-brand-test-{}.ico", std::process::id()));
        let frames = [16, 32].map(|s| RgbaImage::new(s, s));
        write_ico(&path, &frames).expect("write");
        let bytes = std::fs::read(&path).expect("read");
        std::fs::remove_file(&path).ok();
        // ICONDIR: reserved 0, type 1 (icon), frame count.
        assert_eq!(bytes[..6], [0, 0, 1, 0, 2, 0]);
    }
}
