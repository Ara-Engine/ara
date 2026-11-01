//! Moment pyramids of the shadow maps, which make them filterable at any width
//! (`shaders/look-shadow.slangh`: contact-hardening soft shadows). One RG32F image per
//! map kind mirrors it at half resolution and below: the local-light atlas, the
//! cascades (as a 2x2 atlas of cascade-sized tiles) and the far map. Per texel it holds
//! the mean stored depth of the map texels it covers and their variance about that
//! mean. Levels pack side by side (level 0 at the origin, level k >= 1 at
//! `[W, H - H / 2^(k-1)]`), so each level is one storage view.
//!
//! A tile's pyramid is rebuilt (every level, `shaders/look-shadow-moments.slang`) in the
//! frame its depth tile is drawn or patched, and once after the pyramid is created; a
//! still scene builds nothing.
use crate::graph::State;
use anyhow::Result;
use ara_gpu::{Access, Device, GpuDevice, Image, Queue, Resource, vk};

/// The pyramids' format: mean and variance of stored depth.
const FORMAT: vk::Format = vk::Format::R32G32_SFLOAT;
/// Most levels a pyramid builds (a 2048-texel tile's nine).
pub(crate) const MAX_LEVELS: u32 = 9;

/// Levels a tile `side` texels wide builds: down to four texels a side, the
/// deepest a 4x4 lookup window fits (look-shadow.slangh `shadowTop`).
pub(crate) fn levels(side: u32) -> u32 {
    side.max(1).ilog2().saturating_sub(2).min(MAX_LEVELS)
}

/// The pyramid of one map kind: its image and which of its tiles hold a pyramid.
pub(crate) struct Pyramid {
    pub image: Image,
    pub sampled: u32,
    pub storage: u32,
    pub state: State,
    /// Texels of the map (atlas) the pyramid mirrors.
    pub mirror: [u32; 2],
    /// Side of one tile in that map (texels).
    pub tile: u32,
    /// Tiles built since the pyramid was created.
    built: Vec<bool>,
}

impl Pyramid {
    fn new(gpu: &mut GpuDevice, mirror: [u32; 2], tile: u32) -> Result<Self> {
        let size = level_zero(mirror);
        let image = gpu.create_image(
            size[0] + size[0] / 2,
            size[1],
            FORMAT,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
        )?;
        let views = (|| {
            Ok::<_, anyhow::Error>((
                gpu.sampled_image(image, false)?.index,
                gpu.storage_image(image)?.index,
            ))
        })();
        let (sampled, storage) = match views {
            Ok(views) => views,
            Err(error) => {
                gpu.retire(Resource::Image(image))?;
                gpu.collect()?;
                return Err(error);
            }
        };
        let tiles = (mirror[0] / tile) * (mirror[1] / tile);
        Ok(Self {
            image,
            sampled,
            storage,
            state: State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            },
            mirror,
            tile,
            built: vec![false; tiles as usize],
        })
    }

    /// Plans tile `index` (texel origin `origin` in the mirrored map, `depth_origin` in
    /// the depth image `depth`, sampled) if it was drawn this frame (`drawn`), or was
    /// never built and its map holds depth from earlier frames (`held`; an undrawn map
    /// has nothing to read).
    #[allow(clippy::too_many_arguments)]
    fn plan(
        &mut self,
        index: usize,
        drawn: bool,
        held: bool,
        depth: u32,
        origin: [u32; 2],
        depth_origin: [u32; 2],
        builds: &mut Vec<MomentBuild>,
    ) -> bool {
        let Some(built) = self.built.get_mut(index) else {
            return false;
        };
        if !drawn && (*built || !held) {
            return false;
        }
        *built = true;
        let pack = |[x, y]: [u32; 2]| x | y << 16;
        let size = level_zero(self.mirror);
        for level in 0..levels(self.tile) {
            let side = self.tile >> (level + 1);
            builds.push(MomentBuild {
                level,
                words: [
                    depth,
                    self.storage,
                    level,
                    pack(origin),
                    self.tile,
                    pack(depth_origin),
                    pack(size),
                    0,
                ],
                groups: [side.div_ceil(8), side.div_ceil(8), 1],
            });
        }
        true
    }

    fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        Ok(gpu.retire(Resource::Image(self.image))?)
    }
}

/// Level-0 texels of a pyramid mirroring a `mirror`-texel map.
fn level_zero(mirror: [u32; 2]) -> [u32; 2] {
    mirror.map(|v| (v / 2).max(1))
}

/// One level of one tile: a `shadowMomentsMain` dispatch (root words and groups).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MomentBuild {
    pub level: u32,
    pub words: [u32; 8],
    pub groups: [u32; 3],
}

/// This frame's pyramid builds and which pyramids and depth maps they touch.
#[derive(Default)]
pub(crate) struct MomentPlan {
    pub builds: Vec<MomentBuild>,
    /// Pyramids written: local, cascades, far.
    pub written: [bool; 3],
    /// Cascade maps read.
    pub cascades: Vec<usize>,
    pub far: bool,
    pub local: bool,
}

impl MomentPlan {
    /// Levels this frame builds (graph passes `ShadowMoments.0..`).
    pub(crate) fn levels(&self) -> u32 {
        self.builds.iter().map(|b| b.level + 1).max().unwrap_or(0)
    }
}

/// The local, cascade and far pyramids (`LookGpu::moments`).
#[derive(Default)]
pub(crate) struct ShadowMoments {
    pub local: Option<Pyramid>,
    pub cascades: Option<Pyramid>,
    pub far: Option<Pyramid>,
}

/// What this frame's maps hold, for `ShadowMoments::plan`: per map kind its mirrored
/// size and tile side, its depth images (sampled), which tiles were drawn this frame
/// and which held depth before it.
pub(crate) struct MapsDrawn<'a> {
    /// Local atlas texels, tile side, atlas (sampled), drawn faces, whether the atlas
    /// held depth.
    pub local: Option<([u32; 2], u32, u32, &'a [usize], bool)>,
    /// Cascade side, maps (sampled), drawn cascades, which held depth.
    pub cascades: Option<(u32, &'a [u32], &'a [usize], &'a [bool])>,
    /// Far map side, map (sampled), whether it was drawn, whether it held depth.
    pub far: Option<(u32, u32, bool, bool)>,
}

impl ShadowMoments {
    /// Matches the pyramids to the maps (`maps`; a resized or new map gets a fresh
    /// pyramid, a gone one frees its own) and plans this frame's builds.
    pub(crate) fn plan(&mut self, gpu: &mut GpuDevice, maps: &MapsDrawn) -> Result<MomentPlan> {
        let mut plan = MomentPlan::default();
        let local = maps.local.map(|(atlas, tile, ..)| (atlas, tile));
        let cascades = maps.cascades.map(|(side, ..)| ([side * 2; 2], side));
        let far = maps.far.map(|(side, ..)| ([side; 2], side));
        for (slot, want) in [
            (&mut self.local, local),
            (&mut self.cascades, cascades),
            (&mut self.far, far),
        ] {
            if slot.as_ref().map(|p| (p.mirror, p.tile)) != want {
                if let Some(old) = slot.take() {
                    old.destroy(gpu)?;
                }
                if let Some((mirror, tile)) = want {
                    *slot = Some(Pyramid::new(gpu, mirror, tile)?);
                }
            }
        }
        if let (Some(p), Some((atlas, tile, depth, drawn, held))) = (&mut self.local, maps.local) {
            let row = atlas[0] / tile;
            for face in 0..p.built.len() {
                let origin = [face as u32 % row * tile, face as u32 / row * tile];
                let now = drawn.contains(&face);
                if p.plan(face, now, held, depth, origin, origin, &mut plan.builds) {
                    plan.written[0] = true;
                    plan.local = true;
                }
            }
        }
        if let (Some(p), Some((side, depths, drawn, held))) = (&mut self.cascades, maps.cascades) {
            for (c, &depth) in depths.iter().enumerate().take(4) {
                let origin = [(c as u32 & 1) * side, (c as u32 >> 1) * side];
                let (now, held) = (drawn.contains(&c), held.get(c).copied().unwrap_or(false));
                if p.plan(c, now, held, depth, origin, [0, 0], &mut plan.builds) {
                    plan.written[1] = true;
                    plan.cascades.push(c);
                }
            }
        }
        if let (Some(p), Some((_, depth, drawn, held))) = (&mut self.far, maps.far)
            && p.plan(0, drawn, held, depth, [0, 0], [0, 0], &mut plan.builds)
        {
            plan.written[2] = true;
            plan.far = true;
        }
        Ok(plan)
    }

    /// The pyramids in `[local, cascades, far]` order.
    pub(crate) fn pyramids(&self) -> [Option<&Pyramid>; 3] {
        [
            self.local.as_ref(),
            self.cascades.as_ref(),
            self.far.as_ref(),
        ]
    }

    /// Mutable `pyramids`.
    pub(crate) fn pyramids_mut(&mut self) -> [Option<&mut Pyramid>; 3] {
        [
            self.local.as_mut(),
            self.cascades.as_mut(),
            self.far.as_mut(),
        ]
    }

    /// Sampled indices for the look record (`shadowInfo[1]` xyz); zero for none.
    pub(crate) fn indices(&self) -> [u32; 3] {
        self.pyramids().map(|p| p.map_or(0, |p| p.sampled))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_stop_at_four_texel_windows() {
        assert_eq!(levels(256), 6, "128 down to 4");
        assert_eq!(levels(512), 7);
        assert_eq!(levels(2048), 9);
        assert_eq!(levels(8), 1, "one level of four texels");
    }

    #[test]
    fn packed_levels_follow_the_shader_layout() {
        // look-shadow.slangh shadowLevelOrigin / look-shadow-moments.slang levelOrigin.
        let origin = |size: [u32; 2], level: u32| {
            if level == 0 {
                [0, 0]
            } else {
                [size[0], size[1] - (size[1] >> (level - 1))]
            }
        };
        let size = level_zero([1024, 1024]);
        assert_eq!(origin(size, 1), [512, 0]);
        assert_eq!(origin(size, 2), [512, 256]);
        assert_eq!(origin(size, 3), [512, 384]);
        // Level k is (W >> k) wide: every level fits the packed image's width.
        for level in 1..levels(1024) {
            let [x, y] = origin(size, level);
            assert!(x + (size[0] >> level) <= size[0] + size[0] / 2);
            assert!(y + (size[1] >> level) <= size[1]);
        }
    }
}
