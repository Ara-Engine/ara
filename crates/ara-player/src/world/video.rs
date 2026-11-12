//! `play.capture` `video`: a clip of live frames (jitter, temporal resolve and auto
//! exposure running as in Play) while the camera travels and turns linearly (and, with a
//! `Cut`, jumps once), one PNG per 60 Hz frame, for side-by-side comparisons
//! (`ffmpeg -framerate 60 -i %05d.png`).
use anyhow::{Context, Result};
use ara_frame::capture::CaptureRequest;
use ara_runtime::extract::RenderFrame;
use ara_runtime_render::{Presenter, fidelity::native::Content};
use std::path::Path;

/// Still frames rendered at the start pose first, so the resolve and exposure have
/// settled before the clip begins.
const WARMUP: u32 = 30;

/// The camera's move over the clip.
pub(super) struct Clip {
    /// Frames to record.
    pub frames: u32,
    /// World-space travel in metres.
    pub travel: [f32; 3],
    /// Turn about the vertical axis in degrees.
    pub yaw: f32,
    /// One instant cut of the camera inside the clip, if any.
    pub cut: Option<Cut>,
}

/// A jump of the camera from one clip frame on, on top of the linear move and turn: the
/// frame at `at` is the first one rendered from the new pose (a hard cut, no blend).
pub(super) struct Cut {
    /// Clip frame (0-based) of the first frame after the cut.
    pub at: u32,
    /// World-space jump in metres.
    pub travel: [f32; 3],
    /// Turn about the vertical axis in degrees.
    pub yaw: f32,
}

/// The camera's position and forward at loop index `i` (the first `WARMUP` indices hold the
/// start pose): the linear move and turn, plus the cut's jump from its frame on.
fn pose(position: [f32; 3], forward: [f32; 3], clip: &Clip, i: u32) -> ([f32; 3], [f32; 3]) {
    let t = i.saturating_sub(WARMUP) as f32 / clip.frames.saturating_sub(1).max(1) as f32;
    let (jump, turn) = clip
        .cut
        .as_ref()
        .filter(|cut| i >= WARMUP + cut.at)
        .map_or(([0.; 3], 0.), |cut| (cut.travel, cut.yaw));
    let (sin, cos) = (clip.yaw * t + turn).to_radians().sin_cos();
    let [x, y, z] = forward;
    (
        std::array::from_fn(|k| position[k] + clip.travel[k] * t + jump[k]),
        [x * cos + z * sin, y, z * cos - x * sin],
    )
}

/// The clip frames to write, from `ARA_VIDEO_KEEP` (comma-separated 0-based clip frames);
/// `None` (unset) keeps every frame. Frames that are not kept are still rendered and presented
/// but not read back or PNG-encoded. Timing runs need this: reading back and encoding a 1080p
/// frame takes ~100 ms on the render thread, the GPU idles in between and the driver drops its
/// clock (to 210-540 MHz), so GPU timings of a full clip are several times too high.
fn kept_frames() -> Option<Vec<u32>> {
    std::env::var("ARA_VIDEO_KEEP").ok().map(|list| parse_kept(&list))
}

/// The frame numbers in a comma-separated list; entries that are not numbers are ignored.
fn parse_kept(list: &str) -> Vec<u32> {
    list.split(',').filter_map(|frame| frame.trim().parse().ok()).collect()
}

/// Renders the clip at `request.size` into `dir` (`00000.png`, …) from the frame's
/// camera, then restores the presenter's size. With `ARA_VIDEO_KEEP` set, only the listed
/// frames are written.
pub(super) fn record(
    presenter: &mut Presenter,
    frame: &mut RenderFrame,
    request: &CaptureRequest,
    clip: Clip,
    dir: &Path,
    mut content: impl FnMut(&RenderFrame) -> Result<Content>,
) -> Result<()> {
    let start = frame.camera.context("a camera to record from")?;
    std::fs::create_dir_all(dir)?;
    let original = presenter.size();
    presenter.resize(request.size)?;
    let keep = kept_frames();
    let result = (|| {
        for i in 0..WARMUP + clip.frames {
            let camera = frame.camera.as_mut().context("camera")?;
            (camera.position, camera.forward) = pose(start.position, start.forward, &clip, i);
            let frame = &*frame;
            presenter.present_with(frame, || content(frame))?;
            if i >= WARMUP && keep.as_ref().is_none_or(|kept| kept.contains(&(i - WARMUP))) {
                presenter
                    .live_image()
                    .context("live frame")?
                    .save(dir.join(format!("{:05}.png", i - WARMUP)))?;
            }
        }
        Ok(())
    })();
    frame.camera = Some(start);
    presenter.resize(original)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(cut: Option<Cut>) -> Clip {
        Clip {
            frames: 11,
            travel: [10., 0., 0.],
            yaw: 0.,
            cut,
        }
    }

    #[test]
    fn the_keep_list_reads_frame_numbers_and_skips_anything_else() {
        assert_eq!(parse_kept("0, 40,59 ,x,,119"), vec![0, 40, 59, 119]);
        assert!(parse_kept("").is_empty());
    }

    #[test]
    fn a_clip_without_a_cut_moves_linearly() {
        let (position, _) = pose([0.; 3], [0., 0., 1.], &clip(None), WARMUP + 5);
        assert_eq!(position, [5., 0., 0.]);
    }

    #[test]
    fn the_cut_jumps_the_camera_from_its_frame_on_and_not_before() {
        let cut = || {
            Some(Cut {
                at: 4,
                travel: [0., 0., -20.],
                yaw: 90.,
            })
        };
        let before = pose([0.; 3], [0., 0., 1.], &clip(cut()), WARMUP + 3);
        assert!((before.0[0] - 3.).abs() < 1e-5 && before.0[2] == 0. && before.1 == [0., 0., 1.]);
        let (position, forward) = pose([0.; 3], [0., 0., 1.], &clip(cut()), WARMUP + 4);
        assert_eq!(position, [4., 0., -20.]);
        // 90 degrees about the vertical axis turns +Z to +X.
        assert!((forward[0] - 1.).abs() < 1e-6 && forward[2].abs() < 1e-6);
    }

    #[test]
    fn the_warmup_never_cuts() {
        let cut = Some(Cut {
            at: 0,
            travel: [0., 5., 0.],
            yaw: 0.,
        });
        let (position, _) = pose([1., 0., 0.], [0., 0., 1.], &clip(cut), WARMUP - 1);
        assert_eq!(position, [1., 0., 0.]);
    }
}
