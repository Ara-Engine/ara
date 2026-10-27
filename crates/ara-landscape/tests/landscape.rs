use anyhow::{Result, ensure};
use ara_gpu::{
    Device, GpuDevice, Queue, Resource,
    commands::{Attachment, Command},
    cooked::CookedPipeline,
};
use ara_landscape::{Landscape, View, sample};
use std::path::Path;

#[test]
fn noon_dusk_goldens_streamed_pages_and_indirect_contracts() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = root.join("captures/tests");
    std::fs::create_dir_all(&output)?;
    let data = sample::landscape(None)?;
    ensure!(
        data.instances.len() > 1_000_000,
        "scale target not populated"
    );
    let height_hash = blake3::hash(bytemuck::cast_slice(&data.samples))
        .to_hex()
        .to_string();
    let instance_hash = blake3::hash(bytemuck::cast_slice(&data.instances))
        .to_hex()
        .to_string();
    let mut gpu = GpuDevice::new(&[], root.join("cache/test-gpu"))?;
    let mut landscape = Landscape::new(&mut gpu, &data, [2560, 1440], &root.join("cooked"))?;
    ensure!(
        landscape.detail_page_capacity() == 256,
        "unbounded terrain pages"
    );
    let update = std::env::var_os("ARA_UPDATE_LANDSCAPE_GOLDENS").is_some();
    let goldens = root.join("tests/goldens");
    std::fs::create_dir_all(&goldens)?;
    for (name, view) in [("noon", View::noon()), ("dusk", View::dusk())] {
        for _ in 0..4 {
            landscape.update(&gpu, &view)?;
            let commands = landscape.record(&gpu)?;
            let point = gpu.submit(commands, &[])?;
            gpu.resolve(point)?;
        }
        let actual = output.join(format!("{name}.png"));
        landscape.capture(&mut gpu, &actual)?;
        let pixels = image::open(&actual)?.to_rgba8();
        let golden = goldens.join(format!("{name}.png"));
        if update {
            pixels.save(&golden)?;
        }
        let baseline = image::open(&golden)?.to_rgba8();
        let result = ara_render::capture::compare(&pixels, &baseline, 0.005)?;
        if !result.passed {
            result.diff.save(output.join(format!("{name}-diff.png")))?;
        }
        ensure!(
            result.passed,
            "{name} 1-SSIM {} exceeds 0.005",
            result.score
        );
        landscape.update(&gpu, &view)?;
        let commands = landscape.record(&gpu)?;
        let point = gpu.submit(commands, &[])?;
        gpu.resolve(point)?;
        let repeat = output.join(format!("{name}-repeat.png"));
        landscape.capture(&mut gpu, &repeat)?;
        ensure!(
            pixels == image::open(repeat)?.to_rgba8(),
            "capture not deterministic"
        );
    }
    if update {
        ara_gpu::cooked::write_json(
            &goldens.join("provenance.json"),
            &serde_json::json!({"device":gpu.info(),"height_hash":height_hash,"instance_hash":instance_hash,"tick":120,"seed":42,"size":[2560,1440],"instances":data.instances.len(),"tolerance_1_ssim":0.005}),
        )?;
    }
    // Stream a worker-produced changed tile and prove its pixels reach the draw.
    let coord = ara_terrain::TileCoord { x: 14, z: 12 };
    let mut tile = sample::tile(&data.header, coord)?;
    tile.heights.iter_mut().for_each(|h| *h += 35.);
    ensure!(
        !landscape.queue_tile(&gpu, coord, &tile, false)?,
        "D20 policy ignored"
    );
    ensure!(
        landscape.queue_tile(&gpu, coord, &tile, true)?,
        "completed tile rejected"
    );
    ensure!(
        !landscape.queue_tile(&gpu, coord, &tile, true)?,
        "per-frame upload budget exceeded"
    );
    landscape.update(&gpu, &View::noon())?;
    let commands = landscape.record(&gpu)?;
    let point = gpu.submit(commands, &[])?;
    let timings = gpu.resolve(point)?;
    ensure!(
        timings.iter().any(|t| t.pass == "tile_upload"),
        "streamed tile pass missing"
    );
    let changed = output.join("changed.png");
    landscape.capture(&mut gpu, &changed)?;
    ensure!(
        image::open(changed)?.to_rgba8() != image::open(goldens.join("noon.png"))?.to_rgba8(),
        "streamed height edit did not affect pixels"
    );
    gpu.assert_clean()?;
    // Authored mesh vertex pulling, image alpha and billboard LODs share the same
    // instancing/depth/shadow path as the sample's procedural stand-in geometry.
    let geometry = ara_landscape::FoliageGeometry {
        meshes: std::array::from_fn(|_| ara_landscape::FoliageMesh {
            vertices: [
                [-2., 0., 0.],
                [-2., 8., 0.],
                [2., 0., 0.],
                [2., 0., 0.],
                [-2., 8., 0.],
                [2., 8., 0.],
            ]
            .into_iter()
            .enumerate()
            .map(|(i, position)| ara_landscape::FoliageVertex {
                position,
                normal: [0., 0., 1.],
                uv: [[0., 0.], [0., 1.], [1., 0.], [1., 0.], [0., 1.], [1., 1.]][i],
            })
            .collect(),
            texture: Some(0),
            color_cutoff: [1., 1., 1., 0.5],
            billboard: true,
        }),
        textures: vec![ara_render::scene::Texture {
            size: [2, 2],
            rgba: vec![
                220, 30, 10, 255, 220, 30, 10, 0, 220, 30, 10, 255, 220, 30, 10, 0,
            ],
            srgb: true,
        }],
    };
    landscape.install_geometry(&mut gpu, &geometry)?;
    landscape.update(&gpu, &View::noon())?;
    let commands = landscape.record(&gpu)?;
    let point = gpu.submit(commands, &[])?;
    gpu.resolve(point)?;
    landscape.capture(&mut gpu, &output.join("authored-mesh.png"))?;
    gpu.assert_clean()?;

    // The additive GPU hook must reject malformed arguments before native recording.
    let pipeline: CookedPipeline =
        serde_json::from_slice(&std::fs::read(root.join("cooked/sky.json"))?)?;
    let pipeline = gpu.create_pipeline(&pipeline, &root.join("cooked"))?;
    let arguments = gpu.create_buffer(16, ara_gpu::vk::BufferUsageFlags::INDIRECT_BUFFER, true)?;
    for offset in [1, 4, u64::MAX - 3] {
        let mut r = gpu.begin(Queue::Graphics);
        r.push(Command::Pipeline(pipeline));
        r.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: landscape.outputs().color,
                clear: None,
            }],
            depth: None,
            size: [2560, 1440],
        });
        r.push(Command::DrawIndirect { arguments, offset });
        r.push(Command::EndRendering);
        ensure!(
            gpu.submit(r.finish(), &[]).is_err(),
            "malformed indirect offset accepted"
        );
    }
    gpu.retire(Resource::Buffer(arguments))?;
    gpu.retire_pipeline(pipeline)?;
    landscape.retire(&mut gpu)?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
