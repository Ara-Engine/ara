//! Metal 4 GPU tests. Shaders here are runtime-compiled MSL written against the same
//! top-level argument buffer and descriptor-table layout Metal Shader Converter emits
//! for Ara's root signature ([`super::abi`]); engine pipelines only load cooked
//! metallibs. Run with `MTL_DEBUG_LAYER=1 MTL_SHADER_VALIDATION=1` for validation.
use super::*;
use crate::commands::{Attachment, Command, Draw};
use crate::cooked::{RenderState, Stage};
use objc2_foundation::NSString;

pub(super) const ABI: &str = r#"
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

pub(super) const SHADERS: &str = r#"
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
    pub(super) fn msl_pipeline(
        &mut self,
        entries: &[(Stage, &str)],
        state: &RenderState,
        threads: Option<[u32; 3]>,
    ) -> Pipeline {
        self.msl_pipeline_from(SHADERS, entries, state, threads)
    }
    /// [`Self::msl_pipeline`] over other shader source, after the same [`ABI`].
    pub(super) fn msl_pipeline_from(
        &mut self,
        shaders: &str,
        entries: &[(Stage, &str)],
        state: &RenderState,
        threads: Option<[u32; 3]>,
    ) -> Pipeline {
        let native = native::pool(|| {
            let source = NSString::from_str(&format!("{ABI}{shaders}"));
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
    let stats = gpu.submission_stats();
    assert_eq!((stats.queue_submissions, stats.command_lists), (1, 1));
    for (buffer, base) in buffers.iter().zip([1000u32, 5000]) {
        let data = words(&gpu.read(*buffer).expect("read"));
        assert_eq!(data, (0..100).map(|i| base + i).collect::<Vec<_>>());
    }
}

#[test]
fn a_pipeline_is_rebound_on_each_encoder_a_pass_timestamp_opens() {
    let mut gpu = device();
    let fill = gpu.msl_pipeline(
        &[(Stage::Compute, "fill")],
        &RenderState::default(),
        Some([64, 1, 1]),
    );
    let buffers = [storage(&mut gpu, 64), storage(&mut gpu, 64)];
    let slots = buffers.map(|b| gpu.storage_buffer(b).expect("slot"));
    let mut rec = gpu.begin(Queue::Compute);
    rec.push(Command::Pipeline(fill));
    for (index, slot) in slots.iter().enumerate() {
        // Pass timestamps close the compute encoder, so the second dispatch of the same
        // pipeline runs on a new encoder that must have it set again.
        rec.push(Command::BeginPass(format!("pass {index}")));
        rec.push(Command::Constants(RootConstants {
            words: [slot.index, 100 * index as u32, 64, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
        rec.push(Command::EndPass);
    }
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    gpu.resolve(point).expect("resolve");
    for (index, buffer) in buffers.iter().enumerate() {
        let data = words(&gpu.read(*buffer).expect("read"));
        let base = 100 * index as u32;
        assert_eq!(data, (0..64).map(|i| base + i).collect::<Vec<_>>());
    }
}

#[test]
fn back_to_back_pass_timestamps_share_one_write_and_resolve_in_order() {
    let mut gpu = device();
    let fill = gpu.msl_pipeline(
        &[(Stage::Compute, "fill")],
        &RenderState::default(),
        Some([64, 1, 1]),
    );
    let buffer = storage(&mut gpu, 64);
    let slot = gpu.storage_buffer(buffer).expect("slot");
    let mut rec = gpu.begin(Queue::Compute);
    rec.push(Command::Pipeline(fill));
    for index in 0..3 {
        rec.push(Command::BeginPass(format!("pass {index}")));
        rec.push(Command::Constants(RootConstants {
            words: [slot.index, index, 64, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
        rec.push(Command::EndPass);
    }
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    let timings = gpu.resolve(point).expect("resolve");
    assert_eq!(timings.len(), 3);
    for pass in &timings {
        assert!(
            pass.start_ms > 0. && pass.start_ms <= pass.end_ms,
            "{pass:?}"
        );
    }
    // A pass end and the next begin have no work between them: one timestamp.
    for pair in timings.windows(2) {
        assert_eq!(pair[0].end_ms, pair[1].start_ms);
    }
}

#[test]
fn submissions_share_one_timestamp_heap_as_ranges_and_reuse_freed_ones() {
    let gpu = device();
    let mut pool = timestamps::Pool::default();
    // 511 passes take 1024 slots: four ranges fill the heap, a fifth runs untimed.
    let labels = || vec![String::from("pass"); 511];
    let taken: Vec<_> = std::iter::from_fn(|| pool.take(&gpu.device, labels())).collect();
    let bases = taken.iter().map(|timing| timing.base).collect::<Vec<_>>();
    assert_eq!(bases, [0, 1024, 2048, 3072]);
    assert_eq!(timestamps::SLOTS, 4096);
    pool.recycle(&taken[1]);
    assert_eq!(pool.take(&gpu.device, labels()).map(|t| t.base), Some(1024));
}

#[test]
fn apple_unified_memory_is_the_device_local_quality_budget() {
    let gpu = device();
    let budget = gpu.device.recommendedMaxWorkingSetSize() / (1024 * 1024);
    assert!(gpu.info().unified_device_local);
    assert_eq!(gpu.info().quality_memory_mib(), Some(budget));
}

#[test]
fn a_resolve_inside_a_deferred_scope_leaves_its_timings_to_the_completion() {
    let mut gpu = device();
    let fill = gpu.msl_pipeline(
        &[(Stage::Compute, "fill")],
        &RenderState::default(),
        Some([64, 1, 1]),
    );
    let buffer = storage(&mut gpu, 64);
    let slot = gpu.storage_buffer(buffer).expect("slot");
    gpu.begin_deferred_completion().expect("scope");
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginPass("fill".into()));
    rec.push(Command::Pipeline(fill));
    rec.push(Command::Constants(RootConstants {
        words: [slot.index, 0, 64, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Dispatch([1, 1, 1]));
    rec.push(Command::EndPass);
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    assert!(gpu.resolve(point).expect("scoped resolve").is_empty());
    let mut completion = gpu.end_deferred_completion().expect("end");
    let timings = gpu.wait_completion(&mut completion).expect("wait");
    assert_eq!(
        timings.iter().map(|t| t.pass.as_str()).collect::<Vec<_>>(),
        ["fill"]
    );
}

#[test]
fn a_frame_scope_commits_its_lists_in_one_ordered_submission() {
    let mut gpu = device();
    let [fill, twice] = ["fill", "twice"].map(|entry| {
        gpu.msl_pipeline(
            &[(Stage::Compute, entry)],
            &RenderState::default(),
            Some([64, 1, 1]),
        )
    });
    let data = storage(&mut gpu, 64);
    let slot = gpu.storage_buffer(data).expect("slot");
    let before = gpu.submission_stats();
    gpu.begin_deferred_completion_on_current_queue()
        .expect("scope");
    let mut last = None;
    // Each list depends on the previous one's writes: fill, then double twice.
    for pipeline in [fill, twice, twice] {
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::Pipeline(pipeline));
        rec.push(Command::Constants(RootConstants {
            words: [slot.index, 7, 64, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Dispatch([1, 1, 1]));
        last = Some(gpu.submit(rec.finish(), &[]).expect("submit"));
    }
    let last = last.expect("recorded");
    // Recorded work is committed at the scope's end, so it cannot be waited on yet.
    assert!(Device::wait(&gpu, last).is_err());
    let mut completion = gpu.end_deferred_completion().expect("end");
    let after = gpu.submission_stats();
    assert_eq!(
        (
            after.queue_submissions - before.queue_submissions,
            after.command_lists - before.command_lists
        ),
        (1, 3)
    );
    gpu.wait_completion(&mut completion).expect("complete");
    let values = words(&gpu.read(data).expect("read"));
    assert_eq!(values, (0..64).map(|i| (7 + i) * 4).collect::<Vec<_>>());
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
fn a_transient_depth_is_memoryless_and_lives_within_one_scope() {
    let mut gpu = device();
    gpu.msl_clear();
    let transient =
        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT;
    let depth = gpu
        .create_image(8, 8, vk::Format::D32_SFLOAT, transient)
        .expect("transient depth");
    let storage = gpu
        .images
        .get(depth)
        .expect("depth")
        .raw
        .texture
        .storageMode();
    assert_eq!(storage, MTLStorageMode::Memoryless);
    assert!(matches!(
        gpu.create_image(
            8,
            8,
            vk::Format::D32_SFLOAT,
            transient | vk::ImageUsageFlags::SAMPLED
        ),
        Err(GpuError::Contract(_))
    ));
    let color = gpu
        .create_image(
            8,
            8,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("color");
    let colors = gpu.create_readback_buffer(256).expect("color readback");
    // Two scopes: the transient depth is cleared and written in tile memory each time,
    // never loaded or stored (Metal rejects either on a memoryless texture).
    let mut rec = gpu.begin(Queue::Graphics);
    for clear in [Some([0., 0., 1., 1.]), None] {
        rec.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: color,
                clear,
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
    }
    rec.barriers([Barrier {
        resource: Resource::Image(color),
        before: Access::Color,
        after: Access::CopySource,
        handoff: None,
    }]);
    rec.push(Command::ReadImage {
        source: color,
        destination: colors,
    });
    let point = gpu.submit(rec.finish(), &[]).expect("submit");
    gpu.resolve(point).expect("resolve");
    let pixels = words(&gpu.read(colors).expect("read color"));
    assert!(
        pixels.iter().all(|&p| p == 0xffff_0000),
        "color kept across scopes"
    );
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

#[test]
fn ui_pixel_readback_returns_one_id_texel_and_rejects_bad_requests() {
    let mut gpu = device();
    let id = gpu
        .create_image(
            8,
            4,
            vk::Format::R32_SFLOAT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("ID image");
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: id,
            clear: Some([42.5, 0., 0., 0.]),
        }],
        depth: None,
        size: [8, 4],
    });
    rec.push(Command::EndRendering);
    let point = gpu.submit(rec.finish(), &[]).expect("clear");
    gpu.resolve(point).expect("resolve");
    // SAFETY: the cleared image is complete and nothing writes it during the call.
    let texel = unsafe { gpu.read_ui_pixel(id, [7, 3]) }.expect("pick");
    assert_eq!(f32::from_le_bytes(texel), 42.5);
    // SAFETY: as above; the request itself is invalid.
    let outside = unsafe { gpu.read_ui_pixel(id, [8, 0]) };
    assert!(matches!(outside, Err(GpuError::Contract(_))));
    let color = gpu
        .create_image(
            2,
            2,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("color image");
    // SAFETY: an idle image; only its format is wrong.
    let wrong = unsafe { gpu.read_ui_pixel(color, [0, 0]) };
    assert!(matches!(wrong, Err(GpuError::Contract(_))));
}

#[test]
fn a_loading_split_is_an_independent_device_on_the_same_gpu() {
    let mut game = device();
    // SAFETY: nothing was submitted on `game`.
    let mut loading = unsafe { game.split_loading(std::env::temp_dir()) }.expect("split");
    assert!(!game.shares_graphics_queue(&loading));
    assert!(game.shares_graphics_queue(&game));
    assert_eq!(
        game.info().identity,
        loading.info().identity,
        "same physical GPU"
    );
    let buffer = storage(&mut loading, 4);
    loading
        .upload(
            buffer,
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        )
        .expect("upload");
    assert_eq!(loading.read(buffer).expect("read")[..4], [1, 2, 3, 4]);
    let mut rec = game.begin(Queue::Graphics);
    rec.push(Command::BeginPass("used".into()));
    rec.push(Command::EndPass);
    let point = game.submit(rec.finish(), &[]).expect("submit");
    game.resolve(point).expect("resolve");
    // SAFETY: refused before any native work: `game` has submitted.
    let refused = unsafe { game.split_loading(std::env::temp_dir()) };
    assert!(matches!(refused, Err(GpuError::Contract(_))));
}

/// A long dependent loop, so work queued behind it stays unfinished for a while.
const SPIN: &str = r#"
kernel void spin(constant TopLevel& top [[buffer(2)]], uint id [[thread_position_in_grid]]) {
    uint x = id;
    for (uint i = 0; i < top.root[1]; i++) {
        x = x * 1664525u + 1013904223u;
    }
    top.buffers[top.root[0]].data[id] = x;
}
"#;

#[test]
fn wait_for_orders_another_devices_reads_after_unfinished_work() {
    let mut owner = device();
    let mut borrower = device();
    let spin = owner.msl_pipeline_from(
        SPIN,
        &[(Stage::Compute, "spin")],
        &RenderState::default(),
        Some([64, 1, 1]),
    );
    let scratch = storage(&mut owner, 64);
    let slot = owner.storage_buffer(scratch).expect("slot");
    let image = owner
        .create_image(
            4,
            2,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("image");
    // The owner clears the image only after a long spin, and nobody waits for it.
    let mut rec = owner.begin(Queue::Graphics);
    rec.push(Command::Pipeline(spin));
    rec.push(Command::Constants(RootConstants {
        words: [slot.index, 20_000_000, 0, 0, 0, 0, 0, 0],
    }));
    rec.push(Command::Dispatch([1, 1, 1]));
    rec.barriers([Barrier {
        resource: Resource::Buffer(scratch),
        before: Access::Storage,
        after: Access::Storage,
        handoff: None,
    }]);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image,
            clear: Some([1., 0., 0., 1.]),
        }],
        depth: None,
        size: [4, 2],
    });
    rec.push(Command::EndRendering);
    let point = owner.submit(rec.finish(), &[]).expect("spin and clear");
    let adopted = borrower
        .adopt_texture(
            owner.native_image(image).expect("native texture"),
            vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED,
        )
        .expect("adopt");
    let readback = borrower
        .create_readback_buffer(4 * 2 * 4)
        .expect("readback");
    borrower.wait_for(&owner);
    let mut rec = borrower.begin(Queue::Graphics);
    rec.push(Command::ReadImage {
        source: adopted,
        destination: readback,
    });
    let read = borrower.submit(rec.finish(), &[]).expect("read");
    borrower.resolve(read).expect("resolve");
    let pixels = words(&borrower.read(readback).expect("pixels"));
    assert!(
        pixels.iter().all(|&p| p == 0xff00_00ff),
        "the read ran before the owner's clear: {:08x}",
        pixels[0]
    );
    owner.resolve(point).expect("owner");
}

#[test]
fn another_device_adopts_and_reads_a_texture_of_the_same_gpu() {
    let mut owner = device();
    let mut borrower = device();
    let image = owner
        .create_image(
            4,
            2,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("image");
    let mut rec = owner.begin(Queue::Graphics);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image,
            clear: Some([1., 0., 0., 1.]),
        }],
        depth: None,
        size: [4, 2],
    });
    rec.push(Command::EndRendering);
    let point = owner.submit(rec.finish(), &[]).expect("clear");
    owner.resolve(point).expect("resolve");
    let texture = owner.native_image(image).expect("native texture");
    let adopted = borrower
        .adopt_texture(
            texture.clone(),
            vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED,
        )
        .expect("adopt");
    // The owner may retire its handle: the borrower retains the texture.
    Device::retire(&mut owner, Resource::Image(image)).expect("retire");
    let readback = borrower
        .create_readback_buffer(4 * 2 * 4)
        .expect("readback");
    let mut rec = borrower.begin(Queue::Graphics);
    rec.push(Command::ReadImage {
        source: adopted,
        destination: readback,
    });
    let point = borrower.submit(rec.finish(), &[]).expect("read");
    borrower.resolve(point).expect("resolve");
    let pixels = words(&borrower.read(readback).expect("pixels"));
    assert!(
        pixels.iter().all(|&p| p == 0xff00_00ff),
        "{:08x}",
        pixels[0]
    );
    drop(texture);
    assert_eq!(
        super::native::engine_format(MTLPixelFormat::R32Float),
        Some(vk::Format::R32_SFLOAT)
    );
}
