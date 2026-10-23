//! The 2D presentation path: a world whose camera is a `Camera2d` draws through
//! ara-render2d (sprites, flipbooks, tilemaps, 2D lights and shadows) from an
//! immutable fixed-tick extraction (`Scene2d::extract`).
use anyhow::{Context, Result};
use ara_2d::{Camera2d, Sprite, Tilemap};
use ara_render::capture::{Capture, CaptureRequest, ViewMode};
use ara_render2d::{AtlasImage, Renderer2d, Scene2d};
use ara_runtime::assets::{AssetSource, asset_id};
use ara_schema::AssetRef;
use bevy_ecs::world::World;
use std::{collections::BTreeMap, path::PathBuf};

/// The world's 2D camera, when it is a 2D scene.
pub fn camera(world: &mut World) -> Option<Camera2d> {
    world.query::<&Camera2d>().iter(world).next().cloned()
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
    for (key, (reference, linear)) in wanted {
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
        let image = AtlasImage::from_cooked(atlas, artifact("atlas.rgba")?, linear)?;
        out.insert(key, image);
    }
    Ok(out)
}

/// Draws 2D worlds; the renderer is created on first use and updated in place.
pub struct Presenter2d {
    renderer: Option<Renderer2d>,
    cache: PathBuf,
}

impl Presenter2d {
    /// A presenter whose pipeline cache lives in `cache`.
    pub fn new(cache: impl Into<PathBuf>) -> Self {
        Self {
            renderer: None,
            cache: cache.into(),
        }
    }

    /// A deterministic capture of the world at `tick` through its `Camera2d`.
    pub fn capture(
        &mut self,
        world: &mut World,
        atlases: BTreeMap<String, AtlasImage>,
        tick: u64,
        size: [u32; 2],
        marks: Option<usize>,
        seed: u64,
    ) -> Result<Capture> {
        let camera = camera(world).context("the world has no Camera2d")?;
        let scene = Scene2d::extract(world, atlases, tick)?;
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
