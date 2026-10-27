use super::*;
use ara_asset_format::{TextureFormat, write_ktx2};
use ara_gpu::{Barrier, Queue, commands::Command, vk};
use std::time::Instant;

mod ordering;
mod preparation;
mod raster;
mod reuse;
mod views;

enum Probe {
    Color,
    Normal(f32),
    DecodeColor,
}

fn sample(
    gpu: &mut GpuDevice,
    set: &mut NativeResidency,
    pipeline: ara_gpu::Pipeline,
    range: std::ops::Range<u32>,
    lod: f32,
    probe: Probe,
) -> Vec<f32> {
    let (mode, scale) = match probe {
        Probe::Color => (0, 1.0f32),
        Probe::Normal(scale) => (1, scale),
        Probe::DecodeColor => (2, 1.0),
    };
    let count = range.end - range.start;
    let output = gpu
        .create_buffer(
            u64::from(count) * 16,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )
        .unwrap();
    let slot = gpu.storage_buffer(output).unwrap().index;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.barriers([Barrier {
        resource: Resource::Buffer(output),
        before: Access::Undefined,
        after: Access::Storage,
        handoff: None,
    }]);
    commands.push(Command::Pipeline(pipeline));
    commands.push(Command::Constants(ara_gpu::RootConstants {
        words: [
            set.binding().unwrap().buffer_index,
            slot,
            count,
            lod.to_bits(),
            range.start,
            mode,
            scale.to_bits(),
            0,
        ],
    }));
    commands.push(Command::Dispatch([count.div_ceil(64), 1, 1]));
    commands.barriers([Barrier {
        resource: Resource::Buffer(output),
        before: Access::Storage,
        after: Access::HostRead,
        handoff: None,
    }]);
    let point = gpu.submit(commands.finish(), &[]).unwrap();
    set.submitted(point);
    gpu.resolve(point).unwrap();
    let result = gpu
        .read(output)
        .unwrap()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect();
    gpu.retire(Resource::Buffer(output)).unwrap();
    gpu.collect().unwrap();
    result
}

fn source(size: [u32; 2], mips: u32, value: u8) -> Arc<NativeSource> {
    let levels: Vec<_> = (0..mips)
        .map(|mip| {
            let [w, h] = size.map(|v| (v >> mip).max(1));
            let mut bytes = vec![0; (w.div_ceil(4) * h.div_ceil(4) * 8) as usize];
            for block in bytes.as_chunks_mut::<8>().0 {
                block[..2].fill(value);
            }
            bytes
        })
        .collect();
    Arc::new(
        NativeSource::from_cooked(
            write_ktx2(size[0], size[1], TextureFormat::Bc4, &levels)
                .unwrap()
                .into(),
        )
        .unwrap(),
    )
}

fn bank(budget: u64, upload: u64) -> NativeResidency {
    NativeResidency::new(NativeConfig {
        resident_bytes: budget,
        upload_bytes: upload,
    })
    .unwrap()
}

#[test]
fn priorities_keep_every_map_and_mipless_assets_cannot_be_folded() {
    let mut set = bank(80, 24);
    let high = set.register([0; 32], source([8, 8], 4, 64)).unwrap();
    let low = set.register([1; 32], source([8, 8], 4, 128)).unwrap();
    set.request(high, 0, 100.0).unwrap();
    let (actual, requested) = plan::select(&set.entries, 80, &mut set.plan).unwrap();
    assert_eq!((actual, requested), (80, 112));
    assert_eq!(set.plan, [0, 1]);
    assert!(set.request(low, 4, 1.0).is_err());
    assert!(set.request(low, 0, f32::NAN).is_err());
    let mut pinned = bank(31, 8);
    pinned.register([0; 32], source([8, 8], 1, 64)).unwrap();
    assert!(plan::select(&pinned.entries, 31, &mut pinned.plan).is_err());
    assert!(
        NativeResidency::new(NativeConfig {
            resident_bytes: 0,
            upload_bytes: 1
        })
        .is_err()
    );
}

#[test]
fn cooked_identity_deduplicates_at_the_content_boundary() {
    let mut set = bank(100, 100);
    let image = source([8, 8], 4, 64);
    let first = set.register([1; 32], Arc::clone(&image)).unwrap();
    assert_eq!(first, set.register([1; 32], image).unwrap());
    assert_eq!(set.entries.len(), 1);
    assert!(set.register([1; 32], source([4, 4], 1, 64)).is_err());
}

fn blocks(gpu: &mut GpuDevice, resident: Resident) -> Vec<u8> {
    let read = gpu
        .create_buffer(resident.bytes, vk::BufferUsageFlags::TRANSFER_DST, true)
        .unwrap();
    let mut commands = gpu.begin(Queue::Graphics);
    commands.barriers([Barrier {
        resource: Resource::Image(resident.image),
        before: Access::ShaderRead,
        after: Access::CopySource,
        handoff: None,
    }]);
    commands.push(Command::ReadImage {
        source: resident.image,
        destination: read,
    });
    commands.barriers([
        Barrier {
            resource: Resource::Image(resident.image),
            before: Access::CopySource,
            after: Access::ShaderRead,
            handoff: None,
        },
        Barrier {
            resource: Resource::Buffer(read),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        },
    ]);
    let point = gpu.submit(commands.finish(), &[]).unwrap();
    gpu.resolve(point).unwrap();
    let bytes = gpu.read(read).unwrap();
    gpu.retire(Resource::Buffer(read)).unwrap();
    gpu.collect().unwrap();
    bytes
}

#[test]
fn native_gpu_residency_exceeds_128_and_refines_without_losing_material_maps() {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-texture-stream/native-residency");
    let mut gpu = GpuDevice::new(&[], cache).unwrap();
    let cooked = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("cooked");
    let manifest =
        serde_json::from_slice(&std::fs::read(cooked.join("texture_native_probe.json")).unwrap())
            .unwrap();
    let pipeline = gpu.create_pipeline(&manifest, &cooked).unwrap();
    let mut many = bank(1 << 20, 32);
    for index in 0u32..300 {
        let mut key = [0; 32];
        key[..4].copy_from_slice(&index.to_le_bytes());
        many.register(key, source([4, 4], 1, index as u8)).unwrap();
    }
    let started = Instant::now();
    let stats = many.initialize(&mut gpu).unwrap();
    println!(
        "300 native maps: host={:.3}ms GPU={:.6}ms compressed={}B",
        started.elapsed().as_secs_f64() * 1000.0,
        stats.upload_gpu_ms,
        stats.resident_bytes
    );
    assert_eq!(
        (stats.textures, stats.resident_bytes, stats.pending_textures),
        (300, 2400, 0)
    );
    assert_eq!(many.binding().unwrap().textures, 300);
    assert!(
        many.entries
            .iter()
            .any(|e| e.resident.unwrap().row.0[0] > 127)
    );
    for index in [0, 127, 128, 299] {
        assert_eq!(
            blocks(&mut gpu, many.entries[index].resident.unwrap()),
            many.entries[index].source.stage(0).unwrap()
        );
    }
    let pixels = sample(&mut gpu, &mut many, pipeline, 0..300, 0.0, Probe::Color);
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        assert!(
            (pixel[0] - f32::from(index as u8) / 255.0).abs() < 1e-5,
            "map {index}: {pixel:?}"
        );
    }
    let started = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(many.refine(&mut gpu).unwrap().uploaded_bytes, 0);
    }
    println!(
        "idle residency: {:.3}ns/call",
        started.elapsed().as_secs_f64() * 1e9 / 10_000.0
    );
    let old_binding = many.binding().unwrap().buffer_index;
    let oversized = gpu.info().max_image_dimension + 1;
    many.register([254; 32], source([4, 4], 1, 254)).unwrap();
    many.register([255; 32], source([oversized, 1], 1, 255))
        .unwrap();
    assert!(
        many.refine(&mut gpu).is_err(),
        "new maps require a publication boundary"
    );
    assert!(
        many.initialize(&mut gpu).is_err(),
        "invalid image must not publish a table"
    );
    assert_eq!(many.binding().unwrap().buffer_index, old_binding);
    assert_eq!(many.binding().unwrap().textures, 300);
    assert!(
        many.entries[300].resident.is_none(),
        "the successful partial upload is rolled back too"
    );
    assert_eq!(
        blocks(&mut gpu, many.entries[299].resident.unwrap()),
        many.entries[299].source.stage(0).unwrap()
    );
    many.destroy(&mut gpu).unwrap();

    let mut set = bank(80, 24);
    let first = set.register([0; 32], source([8, 8], 4, 64)).unwrap();
    let second = set.register([1; 32], source([8, 8], 4, 128)).unwrap();
    set.request(first, 0, 100.0).unwrap();
    set.initialize(&mut gpu).unwrap();
    assert_eq!(set.resident_mip(first), Some(([8, 8], 0)));
    assert_eq!(set.resident_mip(second), Some(([8, 8], 1)));
    set.request(first, 0, 1.0).unwrap();
    set.request(second, 0, 100.0).unwrap();
    let partial = set.refine(&mut gpu).unwrap();
    assert_eq!(
        (
            partial.resident_bytes,
            partial.uploaded_bytes,
            partial.pending_textures
        ),
        (48, 24, 1)
    );
    assert_eq!(set.resident_mip(first), Some(([8, 8], 1)));
    assert_eq!(set.resident_mip(second), Some(([8, 8], 1)));
    let complete = set.refine(&mut gpu).unwrap();
    assert_eq!(
        (
            complete.resident_bytes,
            complete.uploaded_bytes,
            complete.pending_textures
        ),
        (80, 56, 0)
    );
    assert_eq!(set.resident_mip(second), Some(([8, 8], 0)));
    for entry in &set.entries {
        let resident = entry.resident.unwrap();
        assert_eq!(
            blocks(&mut gpu, resident),
            entry.source.stage(resident.row.0[1]).unwrap()
        );
    }
    set.destroy(&mut gpu).unwrap();

    let mut levels = bank(1000, 1000);
    let bc4 = |value, blocks: usize| [value, value, 0, 0, 0, 0, 0, 0].repeat(blocks);
    let mip_source = Arc::new(
        NativeSource::from_cooked(
            write_ktx2(
                8,
                8,
                TextureFormat::Bc4,
                &[bc4(0, 4), bc4(64, 1), bc4(128, 1), bc4(255, 1)],
            )
            .unwrap()
            .into(),
        )
        .unwrap(),
    );
    let mip = levels.register([0; 32], mip_source).unwrap();
    levels.request(mip, 1, 1.0).unwrap();
    let normal = [bc4(192, 1), bc4(128, 1)].concat();
    levels
        .register(
            [1; 32],
            Arc::new(
                NativeSource::from_cooked(
                    write_ktx2(4, 4, TextureFormat::Bc5, &[normal])
                        .unwrap()
                        .into(),
                )
                .unwrap(),
            ),
        )
        .unwrap();
    levels.initialize(&mut gpu).unwrap();
    for (lod, expected) in [(0.0, 64.0), (1.5, 96.0), (20.0, 255.0)] {
        let value = sample(&mut gpu, &mut levels, pipeline, 0..1, lod, Probe::Color)[0];
        assert!(
            (value - expected / 255.0).abs() < 1e-5,
            "resident mip LOD {lod}: {value}"
        );
    }
    let actual = sample(
        &mut gpu,
        &mut levels,
        pipeline,
        1..2,
        0.0,
        Probe::Normal(2.0),
    );
    let xy = [192.0f32 / 255.0 * 2.0 - 1.0, 128.0 / 255.0 * 2.0 - 1.0];
    let expected = [
        xy[0] * 2.0,
        xy[1] * 2.0,
        (1.0 - xy[0] * xy[0] - xy[1] * xy[1]).sqrt(),
    ];
    let length = expected.iter().map(|v| v * v).sum::<f32>().sqrt();
    for (actual, expected) in actual.iter().zip(expected) {
        assert!((actual - expected / length).abs() < 1e-5);
    }
    levels.destroy(&mut gpu).unwrap();
    gpu.retire_pipeline(pipeline).unwrap();
    gpu.collect().unwrap();
    gpu.assert_clean().unwrap();
}

#[test]
fn mean_texel_decodes_the_coarsest_whole_block_level() {
    let mut set = bank(1 << 20, 1 << 20);
    // BC4 blocks whose endpoints are both 200 decode to a constant red of 200/255.
    let id = set.register([7; 32], source([16, 16], 3, 200)).unwrap();
    let mean = set.mean_texel(id.index(), false).unwrap();
    assert!((mean[0] - 200. / 255.).abs() < 1e-3, "{mean:?}");
    // A material that decodes sRGB itself gets the linear value.
    let decoded = set.mean_texel(id.index(), true).unwrap();
    let linear = ((200. / 255. + 0.055) / 1.055f32).powf(2.4);
    assert!((decoded[0] - linear).abs() < 1e-3, "{decoded:?}");
    assert!(set.mean_texel(id.index() + 1, false).is_none());
    let (size, texels) = set.texels(id.index(), 8, false).unwrap();
    assert_eq!(size, [8, 8], "the finest level within the side limit");
    assert_eq!(texels.len(), 64);
    assert_eq!(
        set.texels(id.index(), 1, false).unwrap().0,
        [4, 4],
        "else the coarsest"
    );
}
