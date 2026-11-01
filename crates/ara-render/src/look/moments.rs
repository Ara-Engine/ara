//! Moment pyramids of the shadow maps, which make them filterable at any width
//! (`shaders/look-shadow.slangh`: contact-hardening soft shadows). One RG32F image per
//! map kind mirrors it at half resolution and below: the local-light atlas, the
//! cascades (as a 2x2 atlas of cascade-sized tiles) and the far map. Per texel it holds
//! the mean stored depth of the map texels it covers, their variance about that mean
//! and the farthest of them from the light (blue; alpha unused). Levels pack side by side (level 0 at the origin, level k >= 1 at
//! `[W, H - H / 2^(k-1)]`), so each level is one storage view.
//!
//! A tile's pyramid is rebuilt (every level, `shaders/look-shadow-moments.slang`) in the
//! frame its depth tile is drawn or patched, and once after the pyramid is created; a
//! still scene builds nothing.
use crate::graph::State;
use anyhow::Result;
use ara_gpu::{Access, Device, GpuDevice, Image, Queue, Resource, vk};

/// The pyramids' format: mean, variance and minimum of stored depth.
const FORMAT: vk::Format = vk::Format::R32G32B32A32_SFLOAT;
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

    /// Plans `tile` (`plan_tile`).
    fn plan(&mut self, tile: TileSource, builds: &mut Vec<MomentBuild>) -> bool {
        plan_tile(
            &mut self.built,
            [self.tile, self.storage],
            self.mirror,
            tile,
            builds,
        )
    }

    fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        Ok(gpu.retire(Resource::Image(self.image))?)
    }
}

/// One map tile this frame: its index among the pyramid's tiles, whether it was drawn
/// this frame or held depth before it, its depth image (sampled) and its texel origins
/// in the mirrored map and in that image.
#[derive(Clone, Copy)]
struct TileSource {
    index: usize,
    drawn: bool,
    held: bool,
    depth: u32,
    origin: [u32; 2],
    depth_origin: [u32; 2],
}

/// Appends every level of `tile` (pyramid tile side and storage index `pyramid`,
/// mirroring a `mirror`-texel map) when it was drawn this frame, or was never built
/// (`built`) and its map held depth (an undrawn map has nothing to read). Returns
/// whether it planned any.
fn plan_tile(
    built: &mut [bool],
    [side, storage]: [u32; 2],
    mirror: [u32; 2],
    tile: TileSource,
    builds: &mut Vec<MomentBuild>,
) -> bool {
    let Some(done) = built.get_mut(tile.index) else {
        return false;
    };
    if !tile.drawn && (*done || !tile.held) {
        return false;
    }
    *done = true;
    let pack = |[x, y]: [u32; 2]| x | y << 16;
    for level in 0..levels(side) {
        let texels = side >> (level + 1);
        builds.push(MomentBuild {
            level,
            words: [
                tile.depth,
                storage,
                level,
                pack(tile.origin),
                side,
                pack(tile.depth_origin),
                pack(level_zero(mirror)),
                0,
            ],
            groups: [texels.div_ceil(8), texels.div_ceil(8), 1],
        });
    }
    true
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

/// This frame's pyramid builds and the tiles they read, per map kind (local,
/// cascades, far; a cascade's index is its map's).
#[derive(Default)]
pub(crate) struct MomentPlan {
    pub builds: Vec<MomentBuild>,
    pub reads: [Vec<usize>; 3],
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

/// One map kind this frame, for `ShadowMoments::plan`: the map (atlas) the pyramid
/// mirrors, its tile side, its depth images (sampled: one shared by every tile, or one
/// per tile), the tiles drawn this frame and whether each image held depth before it.
pub(crate) struct MapDrawn<'a> {
    pub mirror: [u32; 2],
    pub tile: u32,
    pub depths: &'a [u32],
    pub drawn: &'a [usize],
    pub held: &'a [bool],
}

/// The local atlas, cascades and far map this frame.
pub(crate) struct MapsDrawn<'a> {
    pub local: Option<MapDrawn<'a>>,
    pub cascades: Option<MapDrawn<'a>>,
    pub far: Option<MapDrawn<'a>>,
}

impl ShadowMoments {
    /// Matches the pyramids to the maps (`maps`; a resized or new map gets a fresh
    /// pyramid, a gone one frees its own) and plans this frame's builds.
    pub(crate) fn plan(&mut self, gpu: &mut GpuDevice, maps: &MapsDrawn) -> Result<MomentPlan> {
        let mut plan = MomentPlan::default();
        for (kind, (slot, map)) in [
            (&mut self.local, &maps.local),
            (&mut self.cascades, &maps.cascades),
            (&mut self.far, &maps.far),
        ]
        .into_iter()
        .enumerate()
        {
            let want = map.as_ref().map(|m| (m.mirror, m.tile));
            if slot.as_ref().map(|p| (p.mirror, p.tile)) != want {
                if let Some(old) = slot.take() {
                    old.destroy(gpu)?;
                }
                if let Some((mirror, tile)) = want {
                    *slot = Some(Pyramid::new(gpu, mirror, tile)?);
                }
            }
            let (Some(pyramid), Some(map)) = (slot, map) else {
                continue;
            };
            // One shared image holds every tile at its own origin; per-tile images
            // hold theirs at the origin.
            let shared = map.depths.len() == 1;
            let row = map.mirror[0] / map.tile;
            let tiles = if shared {
                pyramid.built.len()
            } else {
                map.depths.len()
            };
            for index in 0..tiles {
                let i = index as u32;
                let origin = [i % row * map.tile, i / row * map.tile];
                let source = TileSource {
                    index,
                    drawn: map.drawn.contains(&index),
                    held: map.held[if shared { 0 } else { index }],
                    depth: map.depths[if shared { 0 } else { index }],
                    origin,
                    depth_origin: if shared { origin } else { [0, 0] },
                };
                if pyramid.plan(source, &mut plan.builds) {
                    plan.reads[kind].push(index);
                }
            }
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
    fn tiles_build_when_drawn_or_first_held() {
        let mut built = vec![false; 2];
        let mut builds = Vec::new();
        let tile = |index, drawn, held| TileSource {
            index,
            drawn,
            held,
            depth: 7,
            origin: [256, 0],
            depth_origin: [256, 0],
        };
        let mut plan = |source, builds: &mut Vec<MomentBuild>| {
            plan_tile(&mut built, [256, 9], [512, 256], source, builds)
        };
        assert!(
            !plan(tile(0, false, false), &mut builds),
            "an undrawn map has nothing to read"
        );
        assert!(
            plan(tile(0, false, true), &mut builds),
            "held depth builds once"
        );
        assert!(
            !plan(tile(0, false, true), &mut builds),
            "then only when drawn"
        );
        assert!(plan(tile(0, true, true), &mut builds));
        assert!(
            !plan(tile(5, true, true), &mut builds),
            "past the pyramid's tiles"
        );
        assert_eq!(builds.len(), 2 * levels(256) as usize);
        // Level 0: 128 texels a side, the tile origin packed, level-0 size 256 x 128.
        assert_eq!(
            builds[0].words,
            [7, 9, 0, 256, 256, 256, 256 | 128 << 16, 0]
        );
        assert_eq!(builds[0].groups, [16, 16, 1]);
        assert_eq!(builds[5].groups, [1, 1, 1], "four texels: one group");
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
