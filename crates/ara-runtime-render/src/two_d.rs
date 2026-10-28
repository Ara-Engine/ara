//! The 2D presentation path: a world whose camera is a `Camera2d` draws through
//! ara-render2d (sprites, flipbooks, tilemaps, 2D lights and shadows) from an
//! immutable fixed-tick extraction (`Scene2d::extract`).
use anyhow::{Context, Result};
use ara_2d::{Atlas, Camera2d, Flipbook, Sprite, SpriteRegion, Tilemap};
use ara_render::capture::{Capture, CaptureRequest, ViewMode};
use ara_render2d::{AtlasImage, Renderer2d, Scene2d};
use ara_runtime::assets::{AssetSource, asset_id};
use ara_schema::AssetRef;
use bevy_ecs::world::World;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

/// The world's 2D camera, when it is a 2D scene.
pub fn camera(world: &mut World) -> Option<Camera2d> {
    world.query::<&Camera2d>().iter(world).next().cloned()
}

/// Resolves a capture camera without modifying any gameplay component.
fn named_camera(world: &mut World, selector: &str) -> Result<Camera2d> {
    let selector = selector.strip_prefix("name:").unwrap_or(selector);
    let mut found = None;
    for (identity, metadata, camera) in world
        .query::<(
            &ara_sim::Identity,
            Option<&ara_sim::SceneMetadata>,
            &Camera2d,
        )>()
        .iter(world)
    {
        if identity.0 != selector && metadata.is_none_or(|m| m.name != selector) {
            continue;
        }
        anyhow::ensure!(
            found.is_none(),
            "camera `{selector}` is ambiguous; use its entity ID"
        );
        found = Some(camera.clone());
    }
    found.with_context(|| format!("no Camera2d matches `{selector}`"))
}

/// Loads every atlas the world's sprites and tilemaps reference from their published
/// import generations (`atlas.json` + `atlas.rgba`), keyed by reference text. Normal
/// atlases decode linear.
pub fn atlases(
    world: &mut World,
    assets: &dyn AssetSource,
) -> Result<BTreeMap<String, AtlasImage>> {
    let mut wanted: BTreeMap<String, (AssetRef, bool)> = BTreeMap::new();
    let mut add = |reference: &Option<AssetRef>, linear: bool| {
        if let Some(r) = reference {
            wanted.insert(r.0.clone(), (r.clone(), linear));
        }
    };
    for sprite in world.query::<&Sprite>().iter(world) {
        add(&sprite.atlas, false);
        add(&sprite.normal_atlas, true);
    }
    for tilemap in world.query::<&Tilemap>().iter(world) {
        add(&tilemap.atlas, false);
        add(&tilemap.normal_atlas, true);
    }
    let mut out = BTreeMap::new();
    let mut regions = BTreeSet::new();
    for sprite in world.query::<&Sprite>().iter(world) {
        regions.insert(sprite.region.clone());
    }
    for clip in world.query::<&Flipbook>().iter(world) {
        regions.extend(clip.frames.iter().map(|f| f.region.clone()));
    }
    for map in world.query::<&Tilemap>().iter(world) {
        regions.extend(map.definitions.values().map(|d| d.region.clone()));
    }
    for (key, (reference, linear)) in wanted {
        let loaded = (|| -> Result<AtlasImage> {
            let id = asset_id(&reference).map_err(|e| anyhow::anyhow!(e.0))?;
            let (_, cached) = assets
                .load(id)
                .map_err(|e| anyhow::anyhow!(e.0))?
                .with_context(|| format!("atlas {key} is not imported"))?;
            let artifact = |name: &str| {
                cached
                    .output
                    .artifacts
                    .iter()
                    .find(|a| a.name == name)
                    .map(|a| a.bytes.as_slice())
                    .with_context(|| format!("atlas {key} has no {name}"))
            };
            let atlas = serde_json::from_slice(artifact("atlas.json")?)?;
            AtlasImage::from_cooked(atlas, artifact("atlas.rgba")?, linear)
        })();
        let image = match loaded {
            Ok(image) => image,
            Err(error) => {
                eprintln!("ARA7303: {error:#}; using a placeholder atlas");
                AtlasImage {
                    atlas: Atlas {
                        width: 16,
                        height: 16,
                        regions: regions
                            .iter()
                            .map(|name| {
                                (
                                    name.clone(),
                                    SpriteRegion {
                                        width: 16,
                                        height: 16,
                                        ..Default::default()
                                    },
                                )
                            })
                            .collect(),
                    },
                    rgba: (0..16 * 16)
                        .flat_map(|i| {
                            if linear {
                                [128, 128, 255, 255]
                            } else if (i / 16 / 4 + i % 16 / 4) % 2 == 0 {
                                [255, 0, 255, 255]
                            } else {
                                [32, 0, 32, 255]
                            }
                        })
                        .collect(),
                    linear,
                }
            }
        };
        out.insert(key, image);
    }
    Ok(out)
}

/// Draws 2D worlds; the renderer is created on first use and updated in place.
pub struct Presenter2d {
    renderer: Option<Renderer2d>,
    cache: PathBuf,
    atlases: BTreeMap<String, AtlasImage>,
    camera: Option<Camera2d>,
}

impl Presenter2d {
    /// A presenter whose pipeline cache lives in `cache`.
    pub fn new(cache: impl Into<PathBuf>) -> Self {
        Self {
            renderer: None,
            cache: cache.into(),
            atlases: BTreeMap::new(),
            camera: None,
        }
    }

    /// Publishes atlas generations once at a frame boundary.
    pub fn set_atlases(&mut self, atlases: BTreeMap<String, AtlasImage>) {
        self.atlases = atlases;
    }

    /// The window renderer and its native presentation device.
    pub fn renderer(&self) -> Option<&Renderer2d> {
        self.renderer.as_ref()
    }

    /// Extracts and uploads the current 2D world; the atlas map moves through the
    /// extraction and back instead of copying texture pixels every frame.
    pub fn prepare(
        &mut self,
        world: &mut World,
        tick: u64,
        size: [u32; 2],
        extensions: &[std::ffi::CString],
    ) -> Result<()> {
        self.camera = Some(camera(world).context("the world has no Camera2d")?);
        let mut scene = Scene2d::extract(world, std::mem::take(&mut self.atlases), tick)?;
        let result = if self.renderer.as_ref().is_none_or(|r| r.size() != size) {
            Renderer2d::with_extensions(&scene, size, &self.cache, extensions).map(|r| {
                self.renderer = Some(r);
            })
        } else {
            self.renderer
                .as_mut()
                .context("2D renderer missing")?
                .update(&scene)
        };
        self.atlases = std::mem::take(&mut scene.atlases);
        result
    }

    /// Draws the prepared view to its native output without capture readback.
    pub fn present(&mut self) -> Result<()> {
        self.renderer
            .as_mut()
            .context("2D renderer missing")?
            .present(self.camera.as_ref().context("2D camera missing")?, 1.0)
    }

    /// A deterministic capture of the world at `tick` through its `Camera2d`.
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        &mut self,
        world: &mut World,
        atlases: BTreeMap<String, AtlasImage>,
        tick: u64,
        size: [u32; 2],
        marks: Option<usize>,
        seed: u64,
        selector: Option<&str>,
    ) -> Result<Capture> {
        let camera = match selector {
            Some(selector) => named_camera(world, selector)?,
            None => camera(world).context("the world has no Camera2d")?,
        };
        let scene = Scene2d::extract(world, atlases, tick)?;
        if self.renderer.as_ref().is_some_and(|r| r.size() != size) {
            self.renderer = None;
        }
        let renderer = match &mut self.renderer {
            Some(renderer) => {
                renderer.update(&scene)?;
                renderer
            }
            slot => slot.insert(Renderer2d::new(&scene, size, &self.cache)?),
        };
        renderer.capture(
            &camera,
            CaptureRequest {
                camera: ara_render::scene::CameraSpec::Orthographic {
                    centre: glam30::Vec2::ZERO,
                    world_size: glam30::Vec2::ONE,
                },
                size,
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 1.0,
                settle_frames: 2,
                tick,
                seed,
                marks,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captures_resolve_2d_cameras_by_name_or_id_and_reject_ambiguity() {
        let mut world = World::new();
        world.spawn((
            ara_sim::Identity("e_camera".into()),
            ara_sim::SceneMetadata {
                name: "Map".into(),
                ..Default::default()
            },
            Camera2d::default(),
        ));
        assert!(named_camera(&mut world, "Map").is_ok());
        assert!(named_camera(&mut world, "e_camera").is_ok());
        assert!(named_camera(&mut world, "missing").is_err());
        world.spawn((
            ara_sim::Identity("e_camera2".into()),
            ara_sim::SceneMetadata {
                name: "Map".into(),
                ..Default::default()
            },
            Camera2d::default(),
        ));
        assert!(named_camera(&mut world, "Map").is_err());
        assert!(named_camera(&mut world, "e_camera").is_ok());
    }
    #[test]
    fn missing_atlas_keeps_sprite_and_flipbook_regions_drawable() {
        let mut world = World::new();
        world.spawn((
            Sprite {
                atlas: Some(AssetRef("@missing.png#as_missing1".into())),
                region: "idle".into(),
                ..Default::default()
            },
            Flipbook {
                frames: vec![ara_2d::FlipbookFrame {
                    region: "walk".into(),
                    ticks: 1,
                }],
                ..Default::default()
            },
        ));
        let images = atlases(&mut world, &ara_runtime::assets::MemoryAssets::default())
            .expect("placeholder atlas");
        let image = &images["@missing.png#as_missing1"];
        assert!(
            image.atlas.regions.contains_key("idle") && image.atlas.regions.contains_key("walk")
        );
        assert_eq!(image.rgba.len(), 16 * 16 * 4);
    }
}
