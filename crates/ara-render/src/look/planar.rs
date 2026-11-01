//! Planar reflections (shaders/look-planar.slang(h), renderer/views/planar.rs): the
//! dominant glossy floor in view reflects a mirror view of the scene, rendered each
//! frame from the camera mirrored below the floor plane, with native draws clipped to
//! what lies above it. Screen-space reflections only reuse what the camera saw, and
//! captures what their points saw, so a floor under hanging pipes reflected the pipes'
//! fronts where it should show the skylights between them; the mirror view sees what
//! the floor sees.
//!
//! The plane comes from the frame itself: the reflection trace counts every fourth
//! half-resolution pixel each way that faces up and is smooth enough by its height
//! below the camera ([`BINS`] bins of [`BIN`]), the counts are read back once their
//! frame completes, and a [`Selector`] keeps one plane until another clearly
//! dominates for several frames (no flicker between a floor and a table). Then the
//! renderer draws the mirror view at [`super::PlanarReflections::scale`] of the render
//! size and copies it into an atlas of [`LEVELS`] filtered levels the trace samples
//! by the reflection lobe's footprint.
use crate::graph::State;
use anyhow::{Context, Result};
use ara_gpu::{Access, Buffer, Device, GpuDevice, Image, Queue, Resource, TimelinePoint, vk};

/// Height histogram bins (look-planar.slangh `PLANAR_BINS`).
pub(crate) const BINS: usize = 4096;
/// Depth below the camera each bin spans (m; `PLANAR_BIN`).
pub(crate) const BIN: f32 = 0.01;
/// Filtered levels below level 0 (`PLANAR_LEVELS`).
pub(crate) const LEVELS: u32 = 6;
/// How far above the floor the mirror view's clip plane lies (m): the floor itself,
/// its seams and its underside stay out of the mirror view.
pub(crate) const LIFT: f32 = 0.01;
/// Histogram words: counts, then offset sums.
const WORDS: usize = BINS * 2;
/// Samples a floor needs to be a plane (of every sixteenth half-resolution pixel).
const MIN_SAMPLES: u32 = 64;
/// Readbacks in a row another floor must dominate before it replaces the plane.
const SWITCH: u32 = 4;
/// Readbacks in a row without the plane in view before it is dropped.
const DROP: u32 = 8;
/// Heights closer than this (m) are the same floor.
const SAME: f32 = 0.02;

/// Chooses the mirror plane from height histograms, with hysteresis: the first floor
/// seen is taken at once; another replaces it only once it has twice the samples
/// for [`SWITCH`] readbacks in a row, or once the plane was out of view for [`DROP`].
/// The plane's height is held while its floor stays in view (no sub-centimetre drift
/// from frame to frame), so the mirror view is still when the camera is.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Selector {
    plane: Option<f32>,
    rival: Option<(f32, u32)>,
    missing: u32,
}

impl Selector {
    /// The chosen plane's height (m).
    pub(crate) fn plane(&self) -> Option<f32> {
        self.plane
    }

    /// One frame's histogram (`counts` then offset sums in 1/256 bins,
    /// look-planar.slangh) taken with the camera at height `camera_y`.
    pub(crate) fn observe(&mut self, words: &[u32], camera_y: f32) {
        let (counts, sums) = words.split_at(BINS.min(words.len()));
        // The fullest bin (the nearest of equals) and its neighbours' mean height.
        let peak = counts
            .iter()
            .enumerate()
            .max_by_key(|&(i, &c)| (c, std::cmp::Reverse(i)))
            .map_or(0, |(i, _)| i);
        let window = peak.saturating_sub(1)..(peak + 2).min(counts.len());
        let count: u32 = counts[window.clone()].iter().sum();
        let candidate = (count >= MIN_SAMPLES).then(|| {
            let depth: f64 = window
                .map(|b| {
                    let sum = sums.get(b).copied().unwrap_or(0);
                    b as f64 * f64::from(counts[b]) + f64::from(sum) / 256.
                })
                .sum::<f64>()
                / f64::from(count);
            (camera_y - depth as f32 * BIN, count)
        });
        // Samples within SAME of a height.
        let support = |height: f32| -> u32 {
            let depth = (camera_y - height) / BIN;
            let reach = SAME / BIN;
            counts
                .iter()
                .enumerate()
                .filter(|&(b, _)| (b as f32 + 0.5 - depth).abs() <= reach)
                .map(|(_, &c)| c)
                .sum()
        };
        let Some(current) = self.plane else {
            self.plane = candidate.map(|(height, _)| height);
            return;
        };
        let held = support(current);
        if held < MIN_SAMPLES {
            self.rival = None;
            self.missing += 1;
            if self.missing >= DROP {
                self.plane = candidate.map(|(height, _)| height);
                self.missing = 0;
            }
            return;
        }
        self.missing = 0;
        self.rival = match (candidate, self.rival) {
            (Some((height, count)), rival)
                if (height - current).abs() > SAME && count > held * 2 =>
            {
                let wins = match rival {
                    Some((h, n)) if (h - height).abs() <= SAME => n + 1,
                    _ => 1,
                };
                if wins >= SWITCH {
                    self.plane = Some(height);
                    None
                } else {
                    Some((height, wins))
                }
            }
            _ => None,
        };
    }
}

/// A height histogram armed for one frame: the camera height its bins are relative
/// to, the frame's submission once it submitted, and the arming order.
#[derive(Clone, Copy)]
struct Armed {
    camera_y: f32,
    order: u64,
    point: Option<TimelinePoint>,
}

/// The mirror view's filtered levels: rgb radiance, a how far the reflected ray
/// travelled (look-planar.slangh layout).
pub(crate) struct Atlas {
    pub image: Image,
    pub sampled: u32,
    pub storage: u32,
    pub state: State,
    /// Level 0's texels (the mirror view's size).
    pub base: [u32; 2],
}

/// Planar reflection state of the primary view: three histogram buffers in flight, the
/// plane [`Selector`] and the atlas.
pub(crate) struct Planar {
    histograms: [(Buffer, u32); 3],
    armed: [Option<Armed>; 3],
    order: u64,
    pub(crate) selector: Selector,
    pub(crate) atlas: Option<Atlas>,
}

impl Planar {
    pub(crate) fn new(gpu: &mut GpuDevice) -> Result<Self> {
        let mut histogram = || -> Result<(Buffer, u32)> {
            let buffer = gpu.create_buffer(
                (WORDS * 4) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )?;
            gpu.upload(buffer, &[0; WORDS * 4])?;
            Ok((buffer, gpu.storage_buffer(buffer)?.index))
        };
        Ok(Self {
            histograms: [histogram()?, histogram()?, histogram()?],
            armed: [None; 3],
            order: 0,
            selector: Selector::default(),
            atlas: None,
        })
    }

    /// Feeds completed histograms to the selector, oldest first (`settle`: waits for
    /// those in flight, so deterministic frames see them at a fixed latency).
    pub(crate) fn observe(&mut self, gpu: &mut GpuDevice, settle: bool) -> Result<()> {
        let completed = gpu.completed()?;
        let mut ready: Vec<usize> = (0..self.armed.len())
            .filter(|&i| self.armed[i].is_some())
            .collect();
        ready.sort_by_key(|&i| self.armed[i].map(|a| a.order));
        for i in ready {
            let Some(armed) = self.armed[i] else {
                continue;
            };
            // Armed by a frame that never submitted: nothing was counted.
            let Some(point) = armed.point else {
                self.armed[i] = None;
                continue;
            };
            if completed[point.queue.index()] < point.value {
                if !settle {
                    // Later histograms wait for this one: the selector sees frames in order.
                    break;
                }
                gpu.wait(point)?;
            }
            self.armed[i] = None;
            let bytes = gpu.read_completed(self.histograms[i].0, point)?;
            let words: &[u32] = bytemuck::try_cast_slice(&bytes)
                .ok()
                .context("planar reflection histogram")?;
            self.selector.observe(&words[..WORDS.min(words.len())], armed.camera_y);
        }
        Ok(())
    }

    /// Arms a cleared histogram (GPU-ordered) for a frame whose camera is at height
    /// `camera_y`; `None` while every buffer still waits to be read.
    pub(crate) fn arm(&mut self, gpu: &mut GpuDevice, camera_y: f32) -> Result<Option<usize>> {
        let Some(i) = self.armed.iter().position(Option::is_none) else {
            return Ok(None);
        };
        gpu.upload_queued(self.histograms[i].0, &[0; WORDS * 4])?;
        self.order += 1;
        self.armed[i] = Some(Armed {
            camera_y,
            order: self.order,
            point: None,
        });
        Ok(Some(i))
    }

    /// Histogram `i`'s buffer and storage index.
    pub(crate) fn histogram(&self, i: usize) -> (Buffer, u32) {
        self.histograms[i]
    }

    /// The frame that counted into histogram `request` submitted at `point`.
    pub(crate) fn submitted(&mut self, request: Option<usize>, point: TimelinePoint) {
        if let Some(armed) = request.and_then(|i| self.armed[i].as_mut()) {
            armed.point = Some(point);
        }
    }

    /// The atlas for a mirror view of `base` texels, reallocated when the size changes.
    pub(crate) fn atlas(&mut self, gpu: &mut GpuDevice, base: [u32; 2]) -> Result<&mut Atlas> {
        if self.atlas.as_ref().is_some_and(|a| a.base != base)
            && let Some(old) = self.atlas.take()
        {
            gpu.retire(Resource::Image(old.image))?;
        }
        if self.atlas.is_none() {
            let image = gpu.create_image(
                base[0] * 2,
                base[1],
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
            )?;
            self.atlas = Some(Atlas {
                image,
                sampled: gpu.sampled_image(image, false)?.index,
                storage: gpu.storage_image(image)?.index,
                state: State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                },
                base,
            });
        }
        Ok(self.atlas.as_mut().expect("allocated above"))
    }

    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        for (buffer, _) in self.histograms {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        if let Some(atlas) = self.atlas {
            gpu.retire(Resource::Image(atlas.image))?;
        }
        Ok(())
    }
}

/// Level `k`'s texels and origin in an atlas whose level 0 has `base` texels
/// (look-planar.slangh `planarSize`, `planarOrigin`).
pub(crate) fn level(base: [u32; 2], k: u32) -> ([u32; 2], [u32; 2]) {
    let size = base.map(|v| (v >> k).max(1));
    let x = if k == 0 {
        0
    } else {
        base[0] + (1..k).map(|j| (base[0] >> j).max(1)).sum::<u32>()
    };
    (size, [x, 0])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A histogram with `samples` at each (height, count) for a camera at `camera_y`.
    fn histogram(camera_y: f32, floors: &[(f32, u32)]) -> Vec<u32> {
        let mut words = vec![0; WORDS];
        for &(height, count) in floors {
            let below = (camera_y - height) / BIN;
            let bin = below as usize;
            words[bin] += count;
            words[BINS + bin] += ((below.fract() * 256.) as u32) * count;
        }
        words
    }

    #[test]
    fn takes_the_first_floor_at_its_height() {
        let mut s = Selector::default();
        s.observe(&histogram(2.0, &[(0.313, 500)]), 2.0);
        let plane = s.plane().expect("a plane");
        assert!((plane - 0.313).abs() < 0.002, "{plane}");
    }

    #[test]
    fn ignores_floors_with_too_few_samples() {
        let mut s = Selector::default();
        s.observe(&histogram(2.0, &[(0.3, MIN_SAMPLES - 1)]), 2.0);
        assert_eq!(s.plane(), None);
    }

    #[test]
    fn holds_the_plane_while_the_camera_moves_and_counts_change() {
        let mut s = Selector::default();
        s.observe(&histogram(2.0, &[(0.31, 500)]), 2.0);
        let plane = s.plane();
        for (camera, count) in [(2.3, 900), (1.7, 300), (2.05, 120)] {
            s.observe(&histogram(camera, &[(0.314, count)]), camera);
            assert_eq!(s.plane(), plane);
        }
    }

    #[test]
    fn switches_only_after_another_floor_dominates_for_several_frames() {
        let mut s = Selector::default();
        s.observe(&histogram(2.0, &[(0.0, 500)]), 2.0);
        let frames = [(0.0, 200), (0.8, 900)];
        for _ in 0..SWITCH - 1 {
            s.observe(&histogram(2.0, &frames), 2.0);
            assert!(s.plane().is_some_and(|h| h.abs() < 0.01));
        }
        // One frame where the old floor holds its own resets the count.
        s.observe(&histogram(2.0, &[(0.0, 600), (0.8, 900)]), 2.0);
        for _ in 0..SWITCH - 1 {
            s.observe(&histogram(2.0, &frames), 2.0);
        }
        assert!(s.plane().is_some_and(|h| h.abs() < 0.01));
        s.observe(&histogram(2.0, &frames), 2.0);
        assert!(s.plane().is_some_and(|h| (h - 0.8).abs() < 0.01));
    }

    #[test]
    fn drops_or_replaces_a_plane_out_of_view_for_long_enough() {
        let mut s = Selector::default();
        s.observe(&histogram(2.0, &[(0.5, 500)]), 2.0);
        for _ in 0..DROP - 1 {
            s.observe(&histogram(2.0, &[]), 2.0);
            assert!(s.plane().is_some());
        }
        s.observe(&histogram(2.0, &[]), 2.0);
        assert_eq!(s.plane(), None);
        s.observe(&histogram(2.0, &[(0.5, 500)]), 2.0);
        for _ in 0..DROP {
            s.observe(&histogram(2.0, &[(1.2, 80)]), 2.0);
        }
        assert!(s.plane().is_some_and(|h| (h - 1.2).abs() < 0.01));
    }

    #[test]
    fn levels_pack_side_by_side_to_the_right_of_level_zero() {
        let base = [100, 60];
        assert_eq!(level(base, 0), ([100, 60], [0, 0]));
        assert_eq!(level(base, 1), ([50, 30], [100, 0]));
        assert_eq!(level(base, 2), ([25, 15], [150, 0]));
        let (size, origin) = level(base, LEVELS);
        assert!(origin[0] + size[0] <= base[0] * 2);
    }
}
