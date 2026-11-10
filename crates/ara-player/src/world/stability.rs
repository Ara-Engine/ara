//! `play.capture` `stability`: how much a still camera's live frames change once the
//! temporal resolve has had `frames` frames to converge. Flicker on thin or distant
//! detail shows as 16x16 blocks whose luminance keeps changing.
use anyhow::{Context, Result};
use ara_render::capture::CaptureRequest;
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::{Presenter, fidelity::native::Content};
use std::path::Path;

/// Consecutive frame pairs measured after the settling frames.
const PAIRS: u32 = 7;

/// Renders `frames` live frames at `request.size` (jitter on), measures the next
/// [`PAIRS`] changes, writes the last frame to `live` (and its flicker map beside it,
/// `.flicker.png`), and restores the presenter's size.
pub(super) fn measure(
    presenter: &mut Presenter,
    frame: &RenderFrame,
    request: &CaptureRequest,
    frames: u32,
    live: &Path,
    mut content: impl FnMut() -> Result<Content>,
) -> Result<ara_play::Stability> {
    let original = presenter.size();
    presenter.resize(request.size)?;
    let result = (|| {
        for _ in 0..frames {
            presenter.present_with(frame, &mut content)?;
        }
        let [width, height] = request.size.map(|v| v as usize);
        let (mut previous, mut sum, mut blocks) = (None::<Vec<f32>>, 0f64, Vec::new());
        // Each pixel's largest change over the pairs, for the flicker map.
        let mut peak = vec![0f32; width * height];
        let mut last = None;
        let mut gpu = Vec::new();
        for _ in 0..=PAIRS {
            presenter.present_with(frame, &mut content)?;
            gpu.push(if presenter.external_renderer().is_some() {
                // An external renderer reports its own GPU span per frame.
                presenter.stats().gpu_ms
            } else {
                presenter
                    .pass_timings()
                    .iter()
                    .map(|t| t.milliseconds)
                    .sum::<f64>()
            });
            let image = presenter.live_image(request).context("live frame")?;
            let luma: Vec<f32> = image
                .pixels()
                .map(|p| {
                    0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])
                })
                .collect();
            if let Some(previous) = &previous {
                let diff: Vec<f32> = luma
                    .iter()
                    .zip(previous)
                    .map(|(a, b)| (a - b).abs())
                    .collect();
                for (peak, d) in peak.iter_mut().zip(&diff) {
                    *peak = peak.max(*d);
                }
                sum += diff.iter().map(|&d| f64::from(d)).sum::<f64>() / diff.len() as f64;
                let columns = width / 16;
                blocks.resize(columns * (height / 16), 0f32);
                for (index, block) in blocks.iter_mut().enumerate() {
                    let (bx, by) = (index % columns * 16, index / columns * 16);
                    let total: f32 = (0..16)
                        .flat_map(|y| (0..16).map(move |x| (y, x)))
                        .map(|(y, x)| diff[(by + y) * width + bx + x])
                        .sum();
                    // Each block keeps its largest mean change over the pairs.
                    *block = block.max(total / 256.);
                }
            }
            previous = Some(luma);
            last = Some(image);
        }
        last.context("live frame")?.save(live)?;
        // Flicker map: black is still; each level of change is 16 levels brighter.
        image::GrayImage::from_fn(width as u32, height as u32, |x, y| {
            image::Luma([(peak[y as usize * width + x as usize] * 16.).min(255.) as u8])
        })
        .save(live.with_extension("flicker.png"))?;
        let flickering = blocks.iter().filter(|&&b| b > 1.).count();
        gpu.sort_by(f64::total_cmp);
        Ok(ara_play::Stability {
            frames,
            mean_change: (sum / f64::from(PAIRS)) as f32,
            worst_block: blocks.iter().copied().fold(0., f32::max),
            flickering_blocks: flickering as f32 / blocks.len().max(1) as f32,
            gpu_ms: gpu.get(gpu.len() / 2).copied().unwrap_or_default() as f32,
            live_image: live.to_string_lossy().into_owned(),
        })
    })();
    presenter.resize(original)?;
    result
}
