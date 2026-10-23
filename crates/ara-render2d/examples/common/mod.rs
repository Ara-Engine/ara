//! Shared sample harness for the platformer example, goldens and benchmark.
#![allow(dead_code)]
pub mod fixtures;
use anyhow::{Result, ensure};
use ara_2d::{Camera2d, Sprite, SpriteRegion};
use ara_render::capture::{CaptureRequest, ViewMode};
use ara_render2d::{AtlasImage, Scene2d, SpriteDraw};
use ara_schema::{AssetRef, SchemaDb};
use std::{collections::BTreeMap, path::Path};

pub const ATLAS: &str = "@art/platformer.png#at_platform1";
pub const NORMAL: &str = "@art/platformer-normal.png#at_normal01";
/// Fixed ticks simulated before capture: the crate lands and the scout settles.
pub const TICKS: u32 = 120;

pub fn request(size: [u32; 2]) -> CaptureRequest {
    CaptureRequest {
        camera: ara_render::scene::CameraSpec::Orthographic {
            centre: glam::Vec2::ZERO,
            world_size: glam::Vec2::ONE,
        },
        size,
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 2,
        tick: u64::from(TICKS),
        seed: 42,
        marks: Some(32),
    }
}
pub fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    ara_components::register(&mut db);
    ara_2d::register(&mut db);
    ara_physics2d::register(&mut db);
    db
}
/// Projects the canonical scene, runs `TICKS` fixed ticks of 2D physics, and extracts the
/// result with boundary-published atlases. Returns the scene's own camera.
pub fn scene(root: &Path, worker: Option<&Path>) -> Result<(Scene2d, Camera2d)> {
    let db = schema();
    let read = ara_doc::read(
        &std::fs::read_to_string(root.join("sample/platformer.scene"))?,
        "platformer.scene",
        &db,
        None,
    );
    ensure!(
        !read.diags.iter().any(|d| d.is_error()),
        "sample diagnostics: {:?}",
        read.diags
    );
    let mut registry = ara_sim::SimRegistry::new(db)?;
    ara_2d::register_sim(&mut registry)?;
    ara_physics2d::register_sim(&mut registry)?;
    let edit = ara_sim::EditWorld::new(
        registry,
        read.doc.ok_or_else(|| anyhow::anyhow!("invalid sample"))?,
    )?;
    let mut session = ara_sim::Session::local(edit, Default::default(), 42, Default::default())?;
    ara_physics2d::install(&mut session, Default::default())?;
    for _ in 0..TICKS {
        session.step()?;
    }
    let world = session.world_mut();
    ensure!(
        world
            .resource::<ara_physics2d::Physics2dStatus>()
            .error
            .is_none(),
        "sample physics faulted"
    );
    let camera = world
        .query::<&Camera2d>()
        .iter(world)
        .next()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("sample has no Camera2d"))?;
    let color = cook_atlas(root, "platformer.png", false, worker)?;
    let normal = cook_atlas(root, "platformer-normal.png", true, worker)?;
    let scene = Scene2d::extract(
        world,
        BTreeMap::from([(ATLAS.into(), color), (NORMAL.into(), normal)]),
        u64::from(TICKS),
    )?;
    Ok((scene, camera))
}
/// Replaces the sample's content with `count` unlit sprites of `size` metres on a grid.
pub fn benchmark(root: &Path, count: u64, size: f32) -> Result<Scene2d> {
    let (mut scene, _) = scene(root, None)?;
    scene.tilemaps.clear();
    scene.lights.clear();
    scene.occluders.clear();
    scene.sprites.clear();
    for i in 0..count {
        scene.sprites.push(SpriteDraw {
            entity: i + 1,
            name: format!("Sprite {i}"),
            position: [(i % 400) as f32 * 0.1 - 20., (i / 400) as f32 * 0.1 - 12.5],
            basis: [1., 0., 0., 1.],
            sprite: Sprite {
                atlas: Some(AssetRef(ATLAS.into())),
                region: "coin".into(),
                size: [size; 2],
                ..Default::default()
            },
        });
    }
    Ok(scene)
}
/// Imports a sample PNG through the out-of-process atlas worker, memoized in a local cache.
fn cook_atlas(
    root: &Path,
    source: &str,
    linear: bool,
    worker: Option<&Path>,
) -> Result<AtlasImage> {
    use ara_asset::{CacheBackend, FileCache, JobEvent, ProcessWorker};
    let input = ara_asset::ImportInput {
        preset: ara_asset::CookPreset::default(),
        thread_budget: ara_asset::DEFAULT_THREAD_BUDGET,
        source_path: source.into(),
        source: std::fs::read(root.join("sample").join(source))?,
        importer: "ara.atlas2d".into(),
        version: "1".into(),
        settings: serde_json::from_slice(&std::fs::read(root.join("sample/atlas.settings.json"))?)?,
        target: ara_asset::TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    };
    let cache = FileCache::at(root.join("../../.sn/2d-fixture-cache"))?;
    let key = input.key()?;
    let output = if let Some(cached) = cache.load(key)? {
        cached.output
    } else {
        let host = if let Some(worker) = worker {
            ProcessWorker::new(worker)
        } else {
            ProcessWorker::new(std::env::current_exe()?).with_arguments(["worker"])
        };
        let mut job = host.start(input)?;
        let output = loop {
            match job.poll()? {
                Some(JobEvent::Complete(output)) => break output,
                _ => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        };
        cache.store(key, &output)?;
        output
    };
    let artifact = |name: &str| {
        output
            .artifacts
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.bytes.as_slice())
            .ok_or_else(|| anyhow::anyhow!("missing atlas artifact {name}"))
    };
    AtlasImage::from_cooked(
        serde_json::from_slice(artifact("atlas.json")?)?,
        artifact("atlas.rgba")?,
        linear,
    )
}
