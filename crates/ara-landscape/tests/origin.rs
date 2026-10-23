//! GPU regression for authored origins, signed tile bounds and empty foliage.
use anyhow::{Result, ensure};
use ara_foliage::Instance;
use ara_gpu::{Device, vulkan::VulkanDevice};
use ara_landscape::{Landscape, View, sample::LandscapeData};
use ara_terrain::{Header, Tile, TileCoord};
use glam::Vec3;
use image::RgbaImage;
use std::path::Path;

fn frame(
    gpu: &mut VulkanDevice,
    landscape: &mut Landscape,
    view: &View,
    path: &Path,
) -> Result<RgbaImage> {
    landscape.update(gpu, view)?;
    let commands = landscape.record(gpu)?;
    let point = gpu.submit(commands, &[])?;
    gpu.resolve(point)?;
    landscape.capture(gpu, path)?;
    Ok(image::open(path)?.to_rgba8())
}

#[test]
fn translated_landscape_matches_origin_view_and_accepts_authored_stream_coordinates() -> Result<()>
{
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("../../target/authoring-tests/landscape-origin");
    std::fs::create_dir_all(&output)?;
    let mut gpu = VulkanDevice::new(&[], output.join("gpu-cache"))?;
    let header = Header {
        origin: [0.; 2],
        spacing: 2.,
        cells: 32,
        min: TileCoord { x: 0, z: 0 },
        max: TileCoord { x: 4, z: 4 },
        materials: Vec::new(),
    };
    let side = 129;
    let samples = (0..side * side)
        .map(|i| {
            let weight = (i % side * 255 / (side - 1)) as u8;
            [
                3f32.to_bits(),
                u32::from_le_bytes([255 - weight, weight, 0, 0]),
                0,
            ]
        })
        .collect::<Vec<_>>();
    let instance = Instance {
        position_scale: [128., 3., 128., 4.],
        rotation_wind: [0., 0., 0., 8.],
        identity: [1, 0, 1, 0],
    };
    let offset = Vec3::new(-3., 0., -108.);
    for populated in [true, false] {
        let data = LandscapeData {
            header: header.clone(),
            side,
            samples: samples.clone(),
            instances: if populated {
                vec![instance]
            } else {
                Vec::new()
            },
        };
        let mut view = View::noon();
        view.eye = Vec3::new(128., 70., 180.);
        view.target = Vec3::new(128., 3., 128.);
        let name = if populated { "foliage" } else { "bare" };
        let mut base = Landscape::new(&mut gpu, &data, [640, 360], &root.join("cooked"))?;
        let expected = frame(
            &mut gpu,
            &mut base,
            &view,
            &output.join(format!("{name}-origin.png")),
        )?;
        let stats = base.visibility_stats(&mut gpu)?;
        ensure!(
            stats.terrain_patches.iter().any(|n| *n > 0),
            "terrain was not drawn"
        );
        ensure!(
            stats.foliage_instances.iter().any(|n| *n > 0) == populated,
            "foliage count contract"
        );
        base.retire(&mut gpu)?;
        gpu.collect()?;

        let mut translated = data;
        translated.header.origin = [125., -300.];
        translated.header.min = TileCoord { x: -2, z: 3 };
        translated.header.max = TileCoord { x: 2, z: 7 };
        for i in &mut translated.instances {
            i.position_scale[0] += offset.x;
            i.position_scale[2] += offset.z;
        }
        view.eye += offset;
        view.target += offset;
        let mut landscape =
            Landscape::new(&mut gpu, &translated, [640, 360], &root.join("cooked"))?;
        let actual = frame(
            &mut gpu,
            &mut landscape,
            &view,
            &output.join(format!("{name}-translated.png")),
        )?;
        ensure!(
            expected == actual,
            "world translation changed landscape pixels"
        );
        let mut tile = Tile::flat(32).map_err(|d| anyhow::anyhow!(d.message))?;
        tile.heights.fill(25.);
        let coord = TileCoord { x: 0, z: 5 };
        ensure!(
            !landscape.queue_tile(&gpu, coord, &tile, false)?,
            "apply policy ignored"
        );
        ensure!(
            landscape.queue_tile(&gpu, coord, &tile, true)?,
            "authored tile rejected"
        );
        let changed = frame(
            &mut gpu,
            &mut landscape,
            &view,
            &output.join(format!("{name}-streamed.png")),
        )?;
        ensure!(
            changed != actual,
            "streamed signed-grid tile did not change pixels"
        );
        landscape.retire(&mut gpu)?;
        gpu.collect()?;
    }
    gpu.assert_clean()?;
    Ok(())
}
