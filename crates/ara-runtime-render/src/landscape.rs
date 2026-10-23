//! Terrain and foliage (ara-landscape) composited into 3D captures by depth. The
//! landscape renders the frame's terrain with its scattered foliage from the capture
//! camera with the scene renderer's projection (infinite reversed-Z, same field of
//! view and near plane), so per pixel the nearer of landscape and scene wins. VFX
//! drawn over the scene carries over onto terrain as its colour delta. Detail pages
//! follow the resident `TerrainTile`s (a coarse 8 m fallback covers the rest).
use anyhow::{Context, Result, ensure};
use ara_gpu::{Access, Device, Queue, Resource, commands::Command, vk};
use ara_landscape::{Landscape, View, sample::LandscapeData};
use ara_render::{
    Renderer,
    capture::{Capture, CaptureRequest},
    scene::CameraSpec,
};
use ara_runtime::{extract::RenderFrame, terrain::TerrainData};
use ara_terrain::TileCoord;
use image::RgbaImage;
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

/// The scene renderer's near plane (`ara_render::scene`).
const NEAR: f32 = 0.1;

fn cooked_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ara-landscape/cooked"
    ))
}

/// One built landscape: for this terrain data at this capture size.
struct Built {
    data: Arc<TerrainData>,
    size: [u32; 2],
    landscape: Landscape,
    queued: BTreeSet<TileCoord>,
}

/// The landscape of the frames' terrain on the renderer's device.
#[derive(Default)]
pub struct Landscapes {
    built: Option<Built>,
}

/// The single sample atlas ara-landscape draws from: heights, packed splat weights
/// and holes over the whole (square, origin-anchored) terrain.
fn landscape_data(data: &TerrainData) -> Result<LandscapeData> {
    let header = &data.header;
    ensure!(
        header.origin == [0.0, 0.0]
            && header.min.x == 0
            && header.min.z == 0
            && header.max.x == header.max.z,
        "the landscape renderer draws square terrains anchored at the origin (min 0, max x == max z)"
    );
    let cells = header.cells as usize;
    let side = header.max.x as usize * cells + 1;
    let mut samples = vec![[0u32; 3]; side * side];
    for (coord, tile) in &data.tiles {
        for tz in 0..=cells {
            for tx in 0..=cells {
                let src = tz * (cells + 1) + tx;
                let x = coord.x as usize * cells + tx;
                let z = coord.z as usize * cells + tz;
                let mut weights = tile.weights[src];
                // The atlas requires weights summing to 255; the remainder goes to 0.
                let sum: u32 = weights.iter().map(|w| u32::from(*w)).sum();
                if sum != 255 {
                    let rest: u32 = weights[1..].iter().map(|w| u32::from(*w)).sum();
                    weights[0] = 255u32.saturating_sub(rest.min(255)) as u8;
                    if rest > 255 {
                        weights = [255, 0, 0, 0];
                    }
                }
                samples[z * side + x] = [
                    tile.heights[src].to_bits(),
                    u32::from_le_bytes(weights),
                    u32::from(tile.holes[src]),
                ];
            }
        }
    }
    Ok(LandscapeData {
        header: header.clone(),
        samples,
        instances: data.foliage.clone(),
        side: side as u32,
    })
}

/// Reads the renderer's reversed-Z depth back (the image must be at `size`).
fn scene_depth(renderer: &mut Renderer, size: [u32; 2]) -> Result<Vec<f32>> {
    let (image, _, state) = renderer.feature_depth();
    let gpu = renderer.gpu_mut();
    let read = gpu.create_buffer(
        u64::from(size[0]) * u64::from(size[1]) * 4,
        vk::BufferUsageFlags::TRANSFER_DST,
        true,
    )?;
    let mut r = gpu.begin(Queue::Graphics);
    r.barriers([ara_gpu::Barrier {
        resource: Resource::Image(image),
        before: state.access,
        after: Access::CopySource,
        handoff: None,
    }]);
    r.push(Command::ReadImage {
        source: image,
        destination: read,
    });
    r.barriers([
        ara_gpu::Barrier {
            resource: Resource::Buffer(read),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        },
        ara_gpu::Barrier {
            resource: Resource::Image(image),
            before: Access::CopySource,
            after: state.access,
            handoff: None,
        },
    ]);
    let p = gpu.submit(r.finish(), &[])?;
    gpu.resolve(p)?;
    let depth = gpu
        .read(read)?
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    gpu.retire(Resource::Buffer(read))?;
    gpu.collect()?;
    Ok(depth)
}

impl Landscapes {
    /// Composites `frame`'s terrain into `capture` (after the scene's VFX): `base` is
    /// the scene image before VFX, so the effects' delta carries over onto terrain.
    /// No terrain: no change.
    pub fn composite(
        &mut self,
        renderer: &mut Renderer,
        frame: &RenderFrame,
        request: &CaptureRequest,
        base: &RgbaImage,
        capture: &mut Capture,
    ) -> Result<()> {
        let Some(terrain) = &frame.terrain else {
            return Ok(());
        };
        let size = request.size;
        let rebuild = self
            .built
            .as_ref()
            .is_none_or(|b| !Arc::ptr_eq(&b.data, &terrain.data) || b.size != size);
        if rebuild {
            if let Some(old) = self.built.take() {
                old.landscape.retire(renderer.gpu_mut())?;
            }
            let data = landscape_data(&terrain.data)?;
            let landscape = Landscape::new(renderer.gpu_mut(), &data, size, &cooked_root())?;
            self.built = Some(Built {
                data: terrain.data.clone(),
                size,
                landscape,
                queued: BTreeSet::new(),
            });
        }
        let built = self.built.as_mut().context("landscape just built")?;
        let CameraSpec::LookAt {
            position,
            target,
            vertical_fov,
        } = &request.camera
        else {
            return Ok(());
        };
        let (sun, sun_color) = match frame.sun {
            Some((direction, lux)) => {
                let peak = lux.iter().copied().fold(1e-6, f32::max);
                (
                    glam30::Vec3::from_array(direction).normalize_or(glam30::Vec3::Y),
                    glam30::Vec3::from_array(lux.map(|c| c / peak)),
                )
            }
            None => (
                glam30::Vec3::new(-0.4, 0.8, -0.25).normalize(),
                glam30::Vec3::ONE,
            ),
        };
        let view = View {
            eye: *position,
            target: *target,
            sun,
            sun_color,
            tick: frame.tick.min(u64::from(u32::MAX)) as u32,
            dt: 1.0 / 60.0,
            fov_y: *vertical_fov,
            near: NEAR,
        };
        // Detail pages: one tile upload per recorded frame (the landscape's budget).
        let mut wanted: Vec<TileCoord> = terrain
            .tiles
            .iter()
            .filter(|c| !built.queued.contains(c))
            .copied()
            .collect();
        wanted.sort();
        loop {
            let gpu = renderer.gpu_mut();
            let mut queued = false;
            if let Some(coord) = wanted.first().copied()
                && let Some(tile) = terrain.data.tiles.get(&coord)
                && built.landscape.queue_tile(gpu, coord, tile, true)?
            {
                built.queued.insert(coord);
                wanted.remove(0);
                queued = true;
            }
            built.landscape.update(gpu, &view)?;
            let list = built.landscape.record(gpu)?;
            let p = gpu.submit(list, &[])?;
            gpu.resolve(p)?;
            if !queued || wanted.is_empty() {
                break;
            }
        }
        let (color, depth) = built.landscape.read_color_depth(renderer.gpu_mut())?;
        let scene = scene_depth(renderer, size)?;
        merge(capture, base, &color, &depth, &scene);
        Ok(())
    }
}

/// Per pixel, the nearer surface (reversed Z: larger is nearer) wins; where terrain
/// wins, the scene's VFX delta (`capture - base`) is added on top. Mark overlays
/// (pixels where the marked image differs from the plain one) stay.
fn merge(capture: &mut Capture, base: &RgbaImage, color: &[u8], depth: &[f32], scene: &[f32]) {
    let plain = capture.image.clone();
    for (i, pixel) in capture.image.pixels_mut().enumerate() {
        let (Some(&l), Some(&s)) = (depth.get(i), scene.get(i)) else {
            continue;
        };
        if l <= s {
            continue;
        }
        let Some(b) = base.as_raw().get(i * 4..i * 4 + 4) else {
            continue;
        };
        for c in 0..3 {
            let delta = i32::from(pixel.0[c]) - i32::from(b[c]);
            pixel.0[c] = (i32::from(color[i * 4 + c]) + delta).clamp(0, 255) as u8;
        }
        pixel.0[3] = 255;
    }
    if let Some(marked) = &mut capture.marked {
        for (i, pixel) in marked.pixels_mut().enumerate() {
            if plain.as_raw().get(i * 4..i * 4 + 4) == Some(&pixel.0[..]) {
                pixel.0 = capture.image.as_raw()[i * 4..i * 4 + 4]
                    .try_into()
                    .unwrap_or(pixel.0);
            }
        }
    }
}
