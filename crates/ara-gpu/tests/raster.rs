#![allow(clippy::unwrap_used)]
//! Raster parity probes run the same draws on Vulkan and Metal.
use ara_gpu::{
    commands::{Attachment, Command},
    cooked::{CookedPipeline, RenderState},
    descriptors::{Format, ImageDesc},
    raster::*,
    sampler::CompareOp,
    texture::*,
    *,
};
fn pipeline(gpu: &mut GpuDevice, name: &str, state: RenderState) -> Pipeline {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cooked");
    let mut desc: CookedPipeline =
        serde_json::from_slice(&std::fs::read(root.join(format!("{name}.json"))).unwrap()).unwrap();
    desc.state = state;
    gpu.create_pipeline(&desc, &root).unwrap()
}
fn image(gpu: &mut GpuDevice, format: Format, samples: SampleCount, source: bool) -> Image {
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
    let mut gpu = GpuDevice::new_raster(&[], std::env::temp_dir()).unwrap();
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
    let p = pipeline(&mut gpu, "streams", state);
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
    rec.barriers([transition(target, Access::Undefined, Access::Color)]);
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
    rec.barriers([Barrier {
        resource: Resource::Buffer(read),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    let point = gpu.submit(rec.finish(), &[]).unwrap();
    gpu.wait(point).unwrap();
    gpu.assert_clean().unwrap();
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
    let mut gpu = GpuDevice::new_raster(&[], std::env::temp_dir()).unwrap();
    for (compare, pass, bias, incoming, slope, gradient) in [
        (CompareOp::Less, true, 0f32, 0.4f32, 0f32, 0f32),
        (CompareOp::LessEqual, true, 0., 0.5, 0., 0.),
        (CompareOp::Equal, true, 0., 0.5, 0., 0.),
        (CompareOp::NotEqual, true, 0., 0.4, 0., 0.),
        (CompareOp::Never, false, 0., 0.4, 0., 0.),
        (CompareOp::Greater, false, 0., 0.4, 0., 0.),
        (CompareOp::Greater, true, 0., 0.6, 0., 0.),
        (CompareOp::GreaterEqual, true, 0., 0.5, 0., 0.),
        (CompareOp::Always, true, 0., 0.4, 0., 0.),
        (CompareOp::Less, false, 0., 0.5, 0., 0.),
        (CompareOp::Less, true, -4096., 0.5, 0., 0.),
        // A tilted surface lies wholly behind 0.5; slope bias brings it in front.
        (CompareOp::Less, false, 0., 0.6, 0., 0.05),
        (CompareOp::Less, true, 0., 0.6, -16., 0.05),
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
                depth_bias: bias,
                depth_bias_slope: slope,
                ..Default::default()
            },
            ..Default::default()
        };
        let p = pipeline(&mut gpu, "fullscreen", state);
        let read = gpu.create_readback_buffer(256).unwrap();
        let mut rec = gpu.begin(Queue::Graphics);
        rec.barriers([
            transition(color, Access::Undefined, Access::Color),
            transition(depth, Access::Undefined, Access::Depth),
            transition(resolved, Access::Undefined, Access::CopyDestination),
        ]);
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
            words: [incoming.to_bits(), gradient.to_bits(), 0, 0, 0, 0, 0, 0],
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
        rec.barriers([Barrier {
            resource: Resource::Buffer(read),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        let point = gpu.submit(rec.finish(), &[]).unwrap();
        gpu.wait(point).unwrap();
        gpu.assert_clean().unwrap();
        let expected = if pass {
            [255, 0, 0, 255]
        } else {
            [0, 0, 0, 255]
        };
        let pixels = gpu.read(read).unwrap();
        let mismatch = pixels.as_chunks::<4>().0.iter().position(|p| *p != expected);
        assert!(
            mismatch.is_none(),
            "{compare:?}: incoming={incoming}, bias={bias}, slope={slope}, gradient={gradient}, expected={expected:?}, first mismatch={:?}",
            mismatch.map(|i| (i, &pixels[i * 4..i * 4 + 4]))
        );
    }
}

#[test]
fn face_winding_culling_and_additive_blend_change_pixels() {
    let mut gpu = GpuDevice::new_raster(&[], std::env::temp_dir()).unwrap();
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
        let p = pipeline(&mut gpu, "fullscreen", state);
        let read = gpu.create_readback_buffer(256).unwrap();
        let mut rec = gpu.begin(Queue::Graphics);
        rec.barriers([transition(target, Access::Undefined, Access::Color)]);
        rec.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: target,
                clear: Some([0., 0., 1., 1.]),
            }],
            depth: None,
            size: [8, 8],
        });
        rec.push(Command::Pipeline(p));
        rec.push(Command::Constants(RootConstants { words: [0; 8] }));
        rec.push(Command::Fullscreen);
        rec.push(Command::EndRendering);
        rec.barriers([transition(target, Access::Color, Access::CopySource)]);
        rec.push(Command::ReadImage {
            source: target,
            destination: read,
        });
        let p = gpu.submit(rec.finish(), &[]).unwrap();
        gpu.wait(p).unwrap();
        gpu.assert_clean().unwrap();
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
