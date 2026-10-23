use ara_schema::{AssetRef, Schema};
use ara_sim::bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Atlas rectangle and authoring geometry, in source pixels.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SpriteRegion {
    /// Pixel left edge.
    pub x: u32,
    /// Pixel top edge.
    pub y: u32,
    /// Source pixel width.
    pub width: u32,
    /// Source pixel height.
    pub height: u32,
    /// Normalized pivot, measured from the bottom left.
    #[ara(default = [0.5, 0.5])]
    pub pivot: [f32; 2],
    /// Nine-slice borders in pixels: left, bottom, right, top.
    pub borders: [f32; 4],
}
impl Default for SpriteRegion {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            pivot: [0.5; 2],
            borders: [0.; 4],
        }
    }
}

/// Canonical atlas metadata; the immutable pixel payload lives in the asset cache.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Atlas {
    /// Atlas pixel width.
    pub width: u32,
    /// Atlas pixel height.
    pub height: u32,
    /// Stable source/subasset names to regions, never heap slots.
    pub regions: BTreeMap<String, SpriteRegion>,
}

/// A batched sprite with optional normal-mapped 2D illumination.
#[derive(Debug, Clone, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "twoD")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Sprite {
    /// Atlas asset reference, resolved to a resident texture at extraction.
    pub atlas: Option<AssetRef>,
    /// Stable region name in the atlas.
    pub region: String,
    /// Optional normal atlas, using the same rectangles.
    pub normal_atlas: Option<AssetRef>,
    /// Desired world size in metres; zero selects source size / pixels per metre.
    pub size: [f32; 2],
    /// Linear, straight-alpha tint.
    #[ara(default = [1, 1, 1, 1])]
    pub tint: [f32; 4],
    /// Painter's sorting layer; lower draws first.
    pub sorting_layer: i32,
    /// Painter's order within a layer; stable identity breaks ties.
    pub order: i32,
    /// Mirror horizontally about the pivot.
    pub flip_x: bool,
    /// Mirror vertically about the pivot.
    pub flip_y: bool,
    /// Apply the region's nine-slice borders when resizing.
    pub nine_slice: bool,
    /// Opt into normal-mapped local lights and geometric shadows.
    pub lit: bool,
    /// Parallax factor; one follows the world, zero follows the camera.
    #[ara(default = [1, 1])]
    pub parallax: [f32; 2],
}
impl Default for Sprite {
    fn default() -> Self {
        Self {
            atlas: None,
            region: String::new(),
            normal_atlas: None,
            size: [0.; 2],
            tint: [1.; 4],
            sorting_layer: 0,
            order: 0,
            flip_x: false,
            flip_y: false,
            nine_slice: false,
            lit: false,
            parallax: [1.; 2],
        }
    }
}

/// Orthographic view. Pixel mode uses integer zoom and nearest texel sampling.
#[derive(Debug, Clone, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "twoD")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Camera2d {
    /// View centre in world metres.
    pub centre: [f32; 2],
    /// Art pixels per world metre.
    #[ara(default = 16, range(min = 0.001))]
    pub pixels_per_metre: f32,
    /// Screen pixels per art pixel, rounded to an integer in pixel mode.
    #[ara(default = 2, range(min = 0.001))]
    pub zoom: f32,
    /// Snap view and sprite geometry to art pixels and sample nearest texels.
    #[ara(default = true)]
    pub pixel_perfect: bool,
    /// Linear scene ambient illumination.
    #[ara(default = [0.2, 0.2, 0.25])]
    pub ambient: [f32; 3],
    /// Linear clear color behind every layer.
    #[ara(default = [0.012, 0.018, 0.032])]
    pub background: [f32; 3],
}
impl Default for Camera2d {
    fn default() -> Self {
        Self {
            centre: [0.; 2],
            pixels_per_metre: 16.,
            zoom: 2.,
            pixel_perfect: true,
            ambient: [0.2, 0.2, 0.25],
            background: [0.012, 0.018, 0.032],
        }
    }
}
impl Camera2d {
    /// Effective pixels per metre and snapped centre for a finite camera.
    pub fn resolved(&self) -> anyhow::Result<(f32, [f32; 2])> {
        anyhow::ensure!(
            self.centre
                .iter()
                .chain(self.ambient.iter())
                .chain(self.background.iter())
                .all(|v| v.is_finite())
                && self.pixels_per_metre.is_finite()
                && self.pixels_per_metre > 0.
                && self.zoom.is_finite()
                && self.zoom > 0.,
            "invalid 2D camera"
        );
        let zoom = if self.pixel_perfect {
            self.zoom.round().max(1.)
        } else {
            self.zoom
        };
        let centre = self.centre.map(|v| {
            if self.pixel_perfect {
                (v * self.pixels_per_metre).round() / self.pixels_per_metre
            } else {
                v
            }
        });
        let scale = self.pixels_per_metre * zoom;
        anyhow::ensure!(
            scale.is_finite()
                && (1. / self.pixels_per_metre).is_finite()
                && centre.iter().all(|v| v.is_finite()),
            "overflowing 2D camera scale/centre"
        );
        Ok((scale, centre))
    }
}

/// Point light for an explicitly lit sprite; Z provides normal-map elevation.
#[derive(Debug, Clone, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "twoD")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Light2d {
    /// Position in world metres.
    pub position: [f32; 3],
    /// Linear RGB intensity.
    #[ara(default = [1, 1, 1])]
    pub color: [f32; 3],
    /// Finite influence radius in metres.
    #[ara(default = 5, range(min = 0.001))]
    pub radius: f32,
    /// Test tile/sprite occluder segments between light and fragment.
    pub shadows: bool,
}
impl Default for Light2d {
    fn default() -> Self {
        Self {
            position: [0., 0., 1.],
            color: [1.; 3],
            radius: 5.,
            shadows: false,
        }
    }
}

/// One deterministic flipbook frame.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FlipbookFrame {
    /// Atlas region name.
    pub region: String,
    /// Frame duration in fixed simulation ticks; must be positive.
    pub ticks: u32,
}
/// Tick-driven flipbook state, wholly contained in an ECS component.
#[derive(Debug, Clone, Default, PartialEq, Schema, Component, Serialize, Deserialize)]
#[ara(component, module = "twoD")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Flipbook {
    /// Ordered clip frames.
    pub frames: Vec<FlipbookFrame>,
    /// Clip's start tick, preserved through snapshots.
    pub start_tick: u64,
    /// Repeat rather than hold the final frame.
    pub looping: bool,
}
impl Flipbook {
    /// Resolves a clip without wall clocks, allocation or mutable hidden state.
    pub fn region_at(&self, tick: u64) -> anyhow::Result<Option<&str>> {
        anyhow::ensure!(
            self.frames.iter().all(|f| f.ticks > 0),
            "zero-duration flipbook frame"
        );
        let total: u64 = self.frames.iter().map(|f| u64::from(f.ticks)).sum();
        if total == 0 {
            return Ok(None);
        }
        let elapsed = tick.saturating_sub(self.start_tick);
        let mut at = if self.looping {
            elapsed % total
        } else {
            elapsed.min(total - 1)
        };
        for frame in &self.frames {
            if at < u64::from(frame.ticks) {
                return Ok(Some(&frame.region));
            }
            at -= u64::from(frame.ticks);
        }
        unreachable!("frame durations sum to total")
    }
}
