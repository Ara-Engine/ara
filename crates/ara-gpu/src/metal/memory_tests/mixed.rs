//! Deterministic mixed 1080p resource inventory, rather than buffer-only padding.
use super::*;
use crate::descriptors::{Format, ImageDesc};
use crate::texture::{ImageShape, TextureDesc, TextureDimension};

enum Spec {
    Buffer(u64, vk::BufferUsageFlags),
    Image(TextureDesc),
}
fn image(size: [u32; 2], format: Format, mip_levels: u32) -> TextureDesc {
    let depth = format == Format::Depth32Float;
    TextureDesc {
        image: ImageDesc {
            size,
            format,
            sampled: true,
            storage: !depth,
            attachment: depth,
            copy_source: false,
            copy_destination: true,
        },
        shape: ImageShape {
            mip_levels,
            ..Default::default()
        },
    }
}
/// 512 mesh pairs, 256 material maps, and 44 full/half/quarter-resolution targets,
/// depth pyramids and volume LUTs. Shapes mirror Renderlab's resource kinds; counts
/// are a reproducible synthetic fixture, not a captured Renderlab scene inventory.
fn inventory() -> Vec<Spec> {
    let mut specs = Vec::new();
    for mesh in 0..512 {
        specs.push(Spec::Buffer(
            (1024 + mesh * 7919 % 3072) * 48,
            vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        ));
        specs.push(Spec::Buffer(
            3 * (512 + mesh * 3571 % 1536) * 4,
            vk::BufferUsageFlags::INDEX_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
        ));
    }
    for _ in 0..256 {
        let mut desc = image([256, 256], Format::Rgba8, 9);
        desc.image.storage = false;
        specs.push(Spec::Image(desc));
    }
    for (count, size, format, mips) in [
        (4, [1920, 1080], Format::Rgba32Float, 1),
        (4, [1920, 1080], Format::Rgba16Float, 1),
        (4, [1920, 1080], Format::R32Float, 1),
        (8, [960, 540], Format::Rgba16Float, 1),
        (4, [960, 540], Format::Rgba32Float, 1),
        (8, [480, 270], Format::Rgba32Float, 1),
        (4, [1920, 1080], Format::Depth32Float, 1),
        (4, [1920, 1080], Format::R32Float, 11),
    ] {
        for _ in 0..count {
            specs.push(Spec::Image(image(size, format, mips)));
        }
    }
    for _ in 0..4 {
        let mut desc = image([32, 32], Format::Rgba16Float, 1);
        desc.shape.dimension = TextureDimension::D3;
        desc.shape.depth = 32;
        desc.image.storage = false;
        specs.push(Spec::Image(desc));
    }
    specs
}
#[derive(serde::Serialize)]
struct Footprint {
    fixture: &'static str,
    resources: usize,
    buffers: usize,
    images: usize,
    requested_bytes: u64,
    unpooled_bytes: u64,
    pooled_bytes: u64,
    heap_count: usize,
}
fn footprint(specs: Vec<Spec>, fixture: &'static str, report_name: &str) {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    assert_eq!(specs.len(), 1324);
    let mut requested = 0;
    let mut raw_buffers = Vec::new();
    let mut raw_images = Vec::new();
    for spec in &specs {
        match spec {
            Spec::Buffer(size, usage) => {
                requested += size;
                raw_buffers.push(resources::buffer(&gpu.device, *size, *usage, false).unwrap());
            }
            Spec::Image(desc) => {
                requested += desc.copy_byte_len().unwrap();
                raw_images.push(
                    resources::image_shaped(
                        &gpu.device,
                        desc.image.size,
                        desc.image.format.vk(),
                        desc.vk_usage(),
                        desc.shape,
                    )
                    .unwrap(),
                );
            }
        }
    }
    eprintln!(
        "mixed separate buffers={} images={}",
        raw_buffers
            .iter()
            .map(|r| MTLAllocation::allocatedSize(&*r.buffer) as u64)
            .sum::<u64>(),
        raw_images
            .iter()
            .map(|r| MTLAllocation::allocatedSize(&*r.texture) as u64)
            .sum::<u64>()
    );
    let unpooled_bytes = raw_buffers
        .iter()
        .map(|r| MTLAllocation::allocatedSize(&*r.buffer) as u64)
        .chain(
            raw_images
                .iter()
                .map(|r| MTLAllocation::allocatedSize(&*r.texture) as u64),
        )
        .sum();
    drop(raw_buffers);
    drop(raw_images);
    let mut buffers = Vec::new();
    let mut images = Vec::new();
    for spec in &specs {
        match spec {
            Spec::Buffer(size, usage) => {
                buffers.push(gpu.create_buffer(*size, *usage, false).unwrap());
            }
            Spec::Image(desc) => images.push(gpu.create_texture(desc).unwrap()),
        }
    }
    let mut heaps = Vec::<memory::Heap>::new();
    for heap in buffers
        .iter()
        .map(|h| gpu.buffers.get(*h).unwrap().raw.heap.as_ref().unwrap())
        .chain(
            images
                .iter()
                .map(|h| gpu.images.get(*h).unwrap().raw.heap.as_ref().unwrap()),
        )
    {
        if !heaps.iter().any(|h| std::ptr::eq(&**h, &**heap)) {
            heaps.push(heap.clone());
        }
    }
    eprintln!(
        "mixed pooled buffer allocations={} image allocations={}",
        buffers
            .iter()
            .map(|h| MTLAllocation::allocatedSize(&*gpu.buffers.get(*h).unwrap().raw.buffer) as u64)
            .sum::<u64>(),
        images
            .iter()
            .map(|h| MTLAllocation::allocatedSize(&*gpu.images.get(*h).unwrap().raw.texture) as u64)
            .sum::<u64>()
    );
    eprintln!(
        "mixed heap live_bytes={} free_bytes={}",
        heaps.iter().map(|h| h.usedSize() as u64).sum::<u64>(),
        heaps
            .iter()
            .map(|h| (h.size() - h.usedSize()) as u64)
            .sum::<u64>()
    );
    let report = Footprint {
        fixture,
        resources: specs.len(),
        buffers: buffers.len(),
        images: images.len(),
        requested_bytes: requested,
        unpooled_bytes,
        pooled_bytes: heaps.iter().map(|h| h.size() as u64).sum(),
        heap_count: heaps.len(),
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../target/ara-gpu/{report_name}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    cooked::write_json(&path, &report).unwrap();
    eprintln!(
        "allocation fixture={} resources={} requested_bytes={} unpooled_bytes={} pooled_bytes={} heaps={} report={}",
        report.fixture,
        report.resources,
        report.requested_bytes,
        report.unpooled_bytes,
        report.pooled_bytes,
        report.heap_count,
        path.display()
    );
}

#[test]
#[ignore = "mixed native allocation footprint; run through ara-dev gpu"]
fn mixed_allocation_footprint() {
    footprint(
        inventory(),
        "mixed-1080p-512-meshes-256-material-maps-v1",
        "mixed-allocation",
    );
}
#[test]
#[ignore = "historical recipe reconstruction; run through ara-dev gpu"]
fn historical_allocation_footprint() {
    // Renderlab docs/gpu-layer-bench.md records the mix, but not its original
    // buffer rounding. Specify our four-byte rounding rather than claiming an
    // exact replay of that executable's resource bytes.
    let mut specs: Vec<_> = (0..1000)
        .map(|i| {
            Spec::Buffer(
                4 * (64. * 4096f64.powf(f64::from(i) / 999.)).round() as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
            )
        })
        .collect();
    for _ in 0..24 {
        let mut desc = image([1920, 1080], Format::Rgba16Float, 1);
        desc.image.copy_destination = false;
        specs.push(Spec::Image(desc));
    }
    for _ in 0..300 {
        let mut desc = image([64, 64], Format::Rgba8, 1);
        desc.image.storage = false;
        desc.image.copy_destination = false;
        specs.push(Spec::Image(desc));
    }
    footprint(
        specs,
        "historical-1324-reconstructed-log4-v1",
        "historical-allocation",
    );
}
