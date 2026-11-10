//! Explicit stream fetch, depth comparison and multisample resolve on real Metal.
use super::*;
use crate::commands::{Attachment, Command};
use crate::cooked::{RenderState, Stage};
use crate::descriptors::{Format, ImageDesc};
use crate::raster::*;
use crate::texture::*;
const SHADER: &str = r#"
struct V { float2 p [[attribute(11)]]; float4 c [[attribute(12)]]; };
struct O { float4 p [[position]]; float4 c; };
vertex O streams(V v [[stage_in]]) {return {float4(v.p,0.4,1),v.c};}
vertex O fullscreen(uint id [[vertex_id]],constant TopLevel& t [[buffer(2)]]) {
    float2 p=float2((id<<1)&2,id&2)*2-1;
    return {float4(p,as_type<float>(t.root[0]),1),float4(1,0,0,1)};
}
fragment float4 color(O v [[stage_in]]) {return v.c;}
"#;
fn image(gpu: &mut MetalDevice, format: Format, samples: SampleCount, source: bool) -> Image {
    gpu.create_texture(&TextureDesc {
        image: ImageDesc {
            size: [8, 8],
            format,
            sampled: false,
            storage: false,
            attachment: true,
            copy_source: source,
            copy_destination: true,
        },
        shape: ImageShape {
            samples,
            ..Default::default()
        },
    })
    .unwrap()
}
fn transition(image: Image, before: Access, after: Access) -> Barrier {
    Barrier {
        resource: Resource::Image(image),
        before,
        after,
        handoff: None,
    }
}
#[test]
fn two_vertex_streams_fetch_per_instance_data_and_nonzero_stream_offsets() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let state = RenderState {
        color_formats: vec![Format::Rgba8.vk().as_raw()],
        raster: RasterState {
            vertex_layouts: vec![
                VertexLayout {
                    stride: 8,
                    step: VertexStep::Vertex,
                    attributes: vec![VertexAttribute {
                        location: 0,
                        format: VertexFormat::Float32x2,
                        offset: 0,
                    }],
                },
                VertexLayout {
                    stride: 16,
                    step: VertexStep::Instance,
                    attributes: vec![VertexAttribute {
                        location: 1,
                        format: VertexFormat::Float32x4,
                        offset: 0,
                    }],
                },
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    let p = gpu.msl_pipeline_from(
        SHADER,
        &[(Stage::Vertex, "streams"), (Stage::Fragment, "color")],
        &state,
        None,
    );
    let mut buffer = |bytes: &[u8], usage| {
        let b = gpu.create_buffer(bytes.len() as u64, usage, true).unwrap();
        gpu.upload(b, bytes).unwrap();
        b
    };
    let positions = buffer(
        bytemuck::cast_slice(&[99f32, 99., -1., -1., 3., -1., -1., 3.]),
        vk::BufferUsageFlags::VERTEX_BUFFER,
    );
    let colors = buffer(
        bytemuck::cast_slice(&[1f32, 0., 0., 1., 0., 1., 0., 1.]),
        vk::BufferUsageFlags::VERTEX_BUFFER,
    );
    let indices = buffer(
        bytemuck::cast_slice(&[0u32, 1, 2]),
        vk::BufferUsageFlags::INDEX_BUFFER,
    );
    let target = image(&mut gpu, Format::Rgba8, SampleCount::One, true);
    let read = gpu.create_readback_buffer(256).unwrap();
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: target,
            clear: Some([0.; 4]),
        }],
        depth: None,
        size: [8, 8],
    });
    rec.push(Command::Pipeline(p));
    rec.push(Command::DrawVertexBuffers {
        vertices: vec![
            VertexBinding {
                buffer: positions,
                offset: 8,
            },
            VertexBinding {
                buffer: colors,
                offset: 0,
            },
        ],
        indices,
        first_index: 0,
        index_count: 3,
        vertex_offset: 0,
        instances: 1,
        first_instance: 1,
    });
    rec.push(Command::EndRendering);
    rec.barriers([transition(target, Access::Color, Access::CopySource)]);
    rec.push(Command::ReadImage {
        source: target,
        destination: read,
    });
    let point = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(point).unwrap();
    assert!(
        gpu.read(read)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 255, 0, 255])
    );
}
#[test]
fn depth_comparisons_and_four_sample_color_resolve() {
    use crate::sampler::CompareOp;
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    for (compare, pass) in [
        (CompareOp::Less, true),
        (CompareOp::LessEqual, true),
        (CompareOp::Greater, false),
        (CompareOp::GreaterEqual, false),
        (CompareOp::Always, true),
    ] {
        let color = image(&mut gpu, Format::Rgba8, SampleCount::Four, true);
        let depth = image(&mut gpu, Format::Depth32Float, SampleCount::Four, false);
        let resolved = image(&mut gpu, Format::Rgba8, SampleCount::One, true);
        let state = RenderState {
            color_formats: vec![Format::Rgba8.vk().as_raw()],
            depth_format: Some(Format::Depth32Float.vk().as_raw()),
            depth_test: true,
            depth_write: true,
            raster: RasterState {
                samples: SampleCount::Four,
                depth_compare: compare,
                ..Default::default()
            },
            ..Default::default()
        };
        let p = gpu.msl_pipeline_from(
            SHADER,
            &[(Stage::Vertex, "fullscreen"), (Stage::Fragment, "color")],
            &state,
            None,
        );
        let read = gpu.create_readback_buffer(256).unwrap();
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: color,
                clear: Some([0., 0., 0., 1.]),
            }],
            depth: Some(Attachment {
                image: depth,
                clear: Some([0.5; 4]),
            }),
            size: [8, 8],
        });
        rec.push(Command::Pipeline(p));
        rec.push(Command::Constants(RootConstants {
            words: [0.4f32.to_bits(), 0, 0, 0, 0, 0, 0, 0],
        }));
        rec.push(Command::Fullscreen);
        rec.push(Command::EndRendering);
        rec.barriers([transition(color, Access::Color, Access::CopySource)]);
        rec.push(Command::ResolveImage {
            source: color,
            destination: resolved,
        });
        rec.barriers([transition(
            resolved,
            Access::CopyDestination,
            Access::CopySource,
        )]);
        rec.push(Command::ReadImage {
            source: resolved,
            destination: read,
        });
        let point = gpu.submit(rec.finish(), &[]).unwrap();
        gpu.wait(point).unwrap();
        let expected = if pass {
            [255, 0, 0, 255]
        } else {
            [0, 0, 0, 255]
        };
        assert!(
            gpu.read(read)
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == expected),
            "{compare:?}"
        );
    }
}

#[test]
fn face_winding_culling_and_additive_blend_change_pixels() {
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    let mut visible = Vec::new();
    for (cull, winding, add) in [
        (CullMode::Back, FrontFace::CounterClockwise, false),
        (CullMode::Front, FrontFace::CounterClockwise, false),
        (CullMode::Back, FrontFace::Clockwise, false),
        (CullMode::None, FrontFace::CounterClockwise, true),
    ] {
        let target = image(&mut gpu, Format::Rgba8, SampleCount::One, true);
        let equation = BlendComponent {
            source: BlendFactor::One,
            destination: BlendFactor::One,
            operation: BlendOp::Add,
        };
        let state = RenderState {
            color_formats: vec![Format::Rgba8.vk().as_raw()],
            raster: RasterState {
                cull,
                front_face: winding,
                blends: if add {
                    vec![Some(BlendState {
                        color: equation,
                        alpha: equation,
                    })]
                } else {
                    vec![]
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let p = gpu.msl_pipeline_from(
            SHADER,
            &[(Stage::Vertex, "fullscreen"), (Stage::Fragment, "color")],
            &state,
            None,
        );
        let read = gpu.create_readback_buffer(256).unwrap();
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: target,
                clear: Some([0., 0., 1., 1.]),
            }],
            depth: None,
            size: [8, 8],
        });
        rec.push(Command::Pipeline(p));
        rec.push(Command::Fullscreen);
        rec.push(Command::EndRendering);
        rec.barriers([transition(target, Access::Color, Access::CopySource)]);
        rec.push(Command::ReadImage {
            source: target,
            destination: read,
        });
        let p = gpu.submit(rec.finish(), &[]).unwrap();
        gpu.wait(p).unwrap();
        let bytes = gpu.read(read).unwrap();
        visible.push(bytes[0] == 255);
        if add {
            assert_eq!(&bytes[..4], &[255, 0, 255, 255]);
        }
    }
    assert_ne!(visible[0], visible[1]);
    assert_eq!(visible[1], visible[2]);
    assert!(visible[3]);
}
