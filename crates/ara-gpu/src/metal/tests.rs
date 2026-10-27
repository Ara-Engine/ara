//! Metal 4 GPU tests. Shaders here are runtime-compiled MSL written against the same
//! top-level argument buffer and descriptor-table layout Metal Shader Converter emits
//! for Ara's root signature ([`super::abi`]); engine pipelines only load cooked
//! metallibs. Run with `MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1` for validation.
use super::*;
use crate::commands::{Attachment, Command, Draw};
use crate::cooked::{RenderState, Stage};
use objc2_foundation::NSString;

const ABI: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct BufferEntry { device uint* data; ulong resource_id; ulong metadata; };
struct TextureEntry { ulong gpu_va; texture2d<float> texture; ulong metadata; };
struct TopLevel {
    uint root[8];
    device TextureEntry* textures;
    device BufferEntry* buffers;
    device TextureEntry* storage_images;
    device BufferEntry* accelerations;
};
"#;

const SHADERS: &str = r#"
kernel void fill(constant TopLevel& top [[buffer(2)]], uint id [[thread_position_in_grid]]) {
    if (id < top.root[2]) {
        top.buffers[top.root[0]].data[id] = top.root[1] + id;
    }
}
kernel void twice(constant TopLevel& top [[buffer(2)]], uint id [[thread_position_in_grid]]) {
    if (id < top.root[2]) {
        device uint* data = top.buffers[top.root[0]].data;
        data[id] = data[id] * 2;
    }
}
struct Vertex {
    float3 position [[attribute(11)]];
    float3 normal [[attribute(12)]];
    float2 uv [[attribute(13)]];
    float4 color [[attribute(14)]];
};
struct Varyings { float4 position [[position]]; float2 uv; float4 color; };
vertex Varyings mesh(Vertex v [[stage_in]]) {
    return { float4(v.position, 1), v.uv, v.color };
}
fragment float4 shade(Varyings in [[stage_in]], constant TopLevel& top [[buffer(2)]]) {
    constexpr sampler nearest(filter::nearest, address::clamp_to_edge);
    float4 texel = top.textures[top.root[0]].texture.sample(nearest, in.uv);
    return texel * in.color;
}
struct ClearOut { float4 position [[position]]; };
vertex ClearOut clearDepthVertex(uint id [[vertex_id]], constant TopLevel& top [[buffer(2)]]) {
    float2 uv = float2((id << 1) & 2, id & 2);
    return { float4(uv * float2(2, -2) + float2(-1, 1), as_type<float>(top.root[0]), 1) };
}
"#;

fn device() -> MetalDevice {
    MetalDevice::new_raster(&[], std::env::temp_dir()).expect("Metal 4 device")
}

impl MetalDevice {
    /// Test-only: a pipeline from runtime-compiled MSL using the converter layout.
    fn msl_pipeline(
        &mut self,
        entries: &[(Stage, &str)],
        state: &RenderState,
        threads: Option<[u32; 3]>,
    ) -> Pipeline {
        let native = native::pool(|| {
            let source = NSString::from_str(&format!("{ABI}{SHADERS}"));
            let library = self
                .device
                .newLibraryWithSource_options_error(&source, None)
                .expect("test MSL compiles");
            let functions = entries
                .iter()
                .map(|(stage, name)| (*stage, &*library, *name))
                .collect::<Vec<_>>();
            pipeline::build(&self.device, &self.compiler, &functions, state, threads)
                .expect("test pipeline")
        });
        self.pipelines.insert(native).expect("pipeline handle")
    }
}

fn storage(gpu: &mut MetalDevice, words: u64) -> Buffer {
    gpu.create_buffer(
        words * 4,
        vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
        true,
    )
    .expect("storage buffer")
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytemuck::cast_slice(bytes).to_vec()
}

#[test]
fn compute_writes_through_the_converter_tables_and_top_level_buffer() {
    let mut gpu = device();
    let fill = gpu.msl_pipeline(
        &[(Stage::Compute, "fill")],
        &RenderState::default(),
        Some([64, 1, 1]),
    );
    let buffers = [storage(&mut gpu, 100), storage(&mut gpu, 100)];
    let slots = buffers.map(|b| gpu.storage_buffer(b).expect("slot"));
    let mut rec = gpu.begin(Queue::Compute);
    rec.push(Command::Pipeline(fill));
    for (slot, base) in slots.iter().zip([1000, 5000]) {
        // Two dispatches with different root words: each must see its own top level.
        rec.push(Command::Constants(RootConstants {
            words: [slot.index, base, 100, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([2, 1, 1]));
    }
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    gpu.resolve(point).expect("resolve");
    for (buffer, base) in buffers.iter().zip([1000u32, 5000]) {
        let data = words(&gpu.read(*buffer).expect("read"));
        assert_eq!(data, (0..100).map(|i| base + i).collect::<Vec<_>>());
    }
}

#[test]
fn graphics_waits_on_the_compute_timeline_and_encoder_barriers_order_dispatches() {
    let mut gpu = device();
    let fill = gpu.msl_pipeline(
        &[(Stage::Compute, "fill")],
        &RenderState::default(),
        Some([64, 1, 1]),
    );
    let twice = gpu.msl_pipeline(
        &[(Stage::Compute, "twice")],
        &RenderState::default(),
        Some([64, 1, 1]),
    );
    let data = storage(&mut gpu, 256);
    let slot = gpu.storage_buffer(data).expect("slot");
    let readback = gpu.create_readback_buffer(1024).expect("readback");
    let root = RootConstants {
        words: [slot.index, 7, 256, 0, 0, 0, 0, 0],
    };
    let mut compute = gpu.begin(Queue::Compute);
    compute.push(Command::Pipeline(fill));
    compute.push(Command::Constants(root));
    compute.push(Command::Dispatch([4, 1, 1]));
    compute.barriers([Barrier {
        resource: Resource::Buffer(data),
        before: Access::Storage,
        after: Access::Storage,
        handoff: None,
    }]);
    compute.push(Command::Pipeline(twice));
    compute.push(Command::Dispatch([4, 1, 1]));
    let written = gpu.submit(compute.finish(), &[]).expect("compute");
    let mut graphics = gpu.begin(Queue::Graphics);
    graphics.push(Command::CopyBuffer {
        source: data,
        destination: readback,
        size: 1024,
    });
    let copied = gpu.submit(graphics.finish(), &[written]).expect("graphics");
    gpu.resolve(copied).expect("resolve");
    let values = words(&gpu.read(readback).expect("read"));
    assert_eq!(values, (0..256).map(|i| (7 + i) * 2).collect::<Vec<_>>());
}

#[test]
fn indexed_draw_fetches_the_48_byte_vertex_abi_and_samples_a_table_texture() {
    let mut gpu = device();
    let size = 64;
    let state = RenderState {
        color_formats: vec![vk::Format::R8G8B8A8_UNORM.as_raw()],
        vertex_stride: 48,
        ..RenderState::default()
    };
    let draw = gpu.msl_pipeline(
        &[(Stage::Vertex, "mesh"), (Stage::Fragment, "shade")],
        &state,
        None,
    );
    // A 2x2 texture: red, green / blue, white; uploaded through a staging buffer.
    let texture = gpu
        .create_image(
            2,
            2,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )
        .expect("texture");
    let texels: [u32; 4] = [0xff00_00ff, 0xff00_ff00, 0xffff_0000, 0xffff_ffff];
    let staging = gpu
        .create_buffer(16, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .expect("staging");
    gpu.upload(staging, bytemuck::cast_slice(&texels))
        .expect("upload");
    let slot = gpu.sampled_image(texture, false).expect("slot");
    // A full-viewport quad as two triangles; color white so the texel passes through.
    let corner = |x: f32, y: f32, u: f32, v: f32| [x, y, 0., 0., 0., 1., u, v, 1., 1., 1., 1.];
    let quad = [
        corner(-1., 1., 0., 0.),
        corner(1., 1., 1., 0.),
        corner(-1., -1., 0., 1.),
        corner(1., -1., 1., 1.),
    ];
    let vertices = gpu
        .create_buffer(48 * 4, vk::BufferUsageFlags::VERTEX_BUFFER, true)
        .expect("vertices");
    gpu.upload(vertices, bytemuck::cast_slice(&quad))
        .expect("vertices");
    let indices = gpu
        .create_buffer(24, vk::BufferUsageFlags::INDEX_BUFFER, true)
        .expect("indices");
    gpu.upload(indices, bytemuck::cast_slice(&[0u32, 1, 2, 2, 1, 3]))
        .expect("indices");
    let target = gpu
        .create_image(
            size,
            size,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("target");
    let readback = gpu
        .create_readback_buffer(u64::from(size * size * 4))
        .expect("readback");
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::UploadImage {
        source: staging,
        destination: texture,
    });
    rec.barriers([Barrier {
        resource: Resource::Image(texture),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: target,
            clear: Some([0., 0., 0., 1.]),
        }],
        depth: None,
        size: [size, size],
    });
    rec.push(Command::Pipeline(draw));
    rec.push(Command::Constants(RootConstants {
        words: [slot.index, 0, 0, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Draw(Draw {
        vertices,
        indices,
        first_index: 0,
        index_count: 6,
    }));
    rec.push(Command::EndRendering);
    rec.barriers([Barrier {
        resource: Resource::Image(target),
        before: Access::Color,
        after: Access::CopySource,
        handoff: None,
    }]);
    rec.push(Command::ReadImage {
        source: target,
        destination: readback,
    });
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    gpu.resolve(point).expect("resolve");
    let pixels = words(&gpu.read(readback).expect("read"));
    let at = |x: u32, y: u32| pixels[(y * size + x) as usize];
    // Top-left quadrant samples texel (0,0): Metal framebuffer y runs downward.
    assert_eq!(at(8, 8), texels[0]);
    assert_eq!(at(56, 8), texels[1]);
    assert_eq!(at(8, 56), texels[2]);
    assert_eq!(at(56, 56), texels[3]);
}

#[test]
fn retired_handles_go_stale_and_slots_free_after_completion() {
    let mut gpu = device();
    let buffer = storage(&mut gpu, 4);
    let slot = gpu.storage_buffer(buffer).expect("slot");
    let point = gpu
        .submit(gpu.begin(Queue::Graphics).finish(), &[])
        .expect("empty submission");
    Device::retire(&mut gpu, Resource::Buffer(buffer)).expect("retire");
    assert!(matches!(gpu.read(buffer), Err(GpuError::StaleHandle)));
    Device::wait(&gpu, point).expect("wait");
    Device::collect(&mut gpu).expect("collect");
    assert_eq!(gpu.descriptor_usage().storage_buffers, 0);
    let replacement = storage(&mut gpu, 4);
    assert_eq!(gpu.storage_buffer(replacement).expect("slot"), slot);
}

impl MetalDevice {
    /// Test-only: the clear pipeline from the MSL twin of `shaders/clear_depth.slang`.
    fn msl_clear(&mut self) {
        let library = native::pool(|| {
            self.device
                .newLibraryWithSource_options_error(
                    &NSString::from_str(&format!("{ABI}{SHADERS}")),
                    None,
                )
                .expect("test MSL compiles")
        });
        self.clear = Some(
            super::clear::Clear::new(&self.device, library, "clearDepthVertex".into())
                .expect("clear pipeline"),
        );
    }
}

#[test]
fn clear_depth_clears_only_its_tile_and_leaves_color_untouched() {
    let mut gpu = device();
    gpu.msl_clear();
    let depth = gpu
        .create_image(
            8,
            8,
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("depth");
    let color = gpu
        .create_image(
            8,
            8,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("color");
    let (depths, colors) = (
        gpu.create_readback_buffer(256).expect("depth readback"),
        gpu.create_readback_buffer(256).expect("color readback"),
    );
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: color,
            clear: Some([0., 0., 1., 1.]),
        }],
        depth: Some(Attachment {
            image: depth,
            clear: Some([1.; 4]),
        }),
        size: [8, 8],
    });
    rec.push(Command::ClearDepth {
        origin: [2, 4],
        size: [4, 2],
        depth: 0.25,
    });
    rec.push(Command::EndRendering);
    for image in [depth, color] {
        rec.barriers([Barrier {
            resource: Resource::Image(image),
            before: Access::Depth,
            after: Access::CopySource,
            handoff: None,
        }]);
    }
    rec.push(Command::ReadImage {
        source: depth,
        destination: depths,
    });
    rec.push(Command::ReadImage {
        source: color,
        destination: colors,
    });
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    gpu.resolve(point).expect("resolve");
    let bytes = gpu.read(depths).expect("read depth");
    let values: &[f32] = bytemuck::cast_slice(&bytes);
    for y in 0..8 {
        for x in 0..8 {
            let tile = (2..6).contains(&x) && (4..6).contains(&y);
            let expected = if tile { 0.25 } else { 1. };
            assert_eq!(values[y * 8 + x], expected, "texel ({x}, {y})");
        }
    }
    let pixels = words(&gpu.read(colors).expect("read color"));
    assert!(
        pixels.iter().all(|&p| p == 0xffff_0000),
        "color stays cleared blue"
    );
    // Outside a depth scope the command is a contract error, as on Vulkan.
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: color,
            clear: None,
        }],
        depth: None,
        size: [8, 8],
    });
    rec.push(Command::ClearDepth {
        origin: [0, 0],
        size: [1, 1],
        depth: 0.,
    });
    rec.push(Command::EndRendering);
    assert!(matches!(
        gpu.submit(rec.finish(), &[]),
        Err(GpuError::Contract(_))
    ));
}

#[test]
fn the_cooked_clear_pipeline_resolves_and_names_missing_converter_artifacts() {
    let gpu = device();
    // Until the canonical host cooks metallibs, loading names the missing lane;
    // afterwards the same manifest loads.
    match super::clear::Clear::cooked(&gpu.device) {
        Ok(_) => {}
        Err(GpuError::Contract(message)) => {
            assert!(message.contains("no Metal artifacts"), "{message}")
        }
        Err(other) => panic!("cooked clear_depth must resolve: {other}"),
    }
}
