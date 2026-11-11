#![allow(clippy::unwrap_used)]
//! Real attachment draws, resolves and depth writes must preserve sibling mips/slices.
use ara_gpu::{
    commands::Command,
    descriptors::{Format, ImageDesc},
    rendering::{RenderAttachment, RenderTarget},
    texture::{ImageShape, SampleCount, TextureDesc},
    *,
};
fn edge(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
fn desc(format: Format) -> TextureDesc {
    TextureDesc {
        image: ImageDesc {
            size: [8, 8],
            format,
            sampled: true,
            storage: false,
            attachment: true,
            copy_source: true,
            copy_destination: true,
        },
        shape: ImageShape {
            layers: 2,
            mip_levels: 3,
            ..Default::default()
        },
    }
}
#[test]
fn raster_mips_layers_and_resolves_preserve_the_other_slices() {
    let mut gpu = GpuDevice::new_raster(
        &[],
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-gpu/render-targets"),
    )
    .unwrap();
    for kind in 0..3 {
        let d = desc(if kind == 1 {
            Format::Depth32Float
        } else {
            Format::Rgba8
        });
        let bytes = d.copy_byte_len().unwrap();
        let image = gpu.create_texture(&d).unwrap();
        let input = gpu
            .create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_SRC, true)
            .unwrap();
        gpu.upload(input, &vec![0; bytes as usize]).unwrap();
        let output = gpu.create_readback_buffer(bytes).unwrap();
        let target = RenderTarget {
            image,
            mip: if kind == 1 { 2 } else { 1 },
            layer: 1,
        };
        let mut rec = gpu.begin(Queue::Graphics);
        rec.barriers([
            edge(
                Resource::Buffer(input),
                Access::HostWrite,
                Access::CopySource,
            ),
            edge(
                Resource::Image(image),
                Access::Undefined,
                Access::CopyDestination,
            ),
        ]);
        rec.push(Command::UploadImage {
            source: input,
            destination: image,
        });
        rec.barriers([Barrier {
            resource: Resource::Image(image),
            before: Access::CopyDestination,
            after: if kind == 1 {
                Access::Depth
            } else {
                Access::Color
            },
            handoff: None,
        }]);
        let mut source = None;
        if kind == 2 {
            let ms = gpu
                .create_texture(&TextureDesc {
                    image: ImageDesc {
                        size: [4, 4],
                        format: Format::Rgba8,
                        sampled: false,
                        storage: false,
                        attachment: true,
                        copy_source: false,
                        copy_destination: false,
                    },
                    shape: ImageShape {
                        layers: 2,
                        samples: SampleCount::Four,
                        transient: true,
                        ..Default::default()
                    },
                })
                .unwrap();
            rec.barriers([edge(Resource::Image(ms), Access::Undefined, Access::Color)]);
            source = Some(ms);
        }
        let attachment = RenderAttachment {
            target: source
                .map(|image| RenderTarget {
                    image,
                    mip: 0,
                    layer: 1,
                })
                .unwrap_or(target),
            clear: Some(if kind == 1 {
                [0.25, 0., 0., 0.]
            } else {
                [0., 1., 0., 1.]
            }),
        };
        rec.push(Command::BeginRenderingSubresources {
            colors: if kind == 1 { vec![] } else { vec![attachment] },
            depth: if kind == 1 { Some(attachment) } else { None },
            resolves: if kind == 2 {
                vec![Some(target)]
            } else {
                vec![]
            },
            size: if kind == 1 { [2, 2] } else { [4, 4] },
        });
        let cooked = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/feature_cooked");
        let name = if kind == 1 { "depth" } else { "color" };
        let mut metadata: cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(cooked.join(format!("{name}.json"))).unwrap())
                .unwrap();
        if kind == 2 {
            metadata.state.raster.samples = SampleCount::Four;
        }
        let p = gpu.create_pipeline(&metadata, &cooked).unwrap();
        rec.push(Command::Pipeline(p));
        rec.push(Command::Fullscreen);
        rec.push(Command::EndRendering);
        if kind == 0 {
            rec.barriers([edge(Resource::Image(image), Access::Color, Access::Color)]);
            rec.push(Command::BeginRenderingSubresources {
                colors: vec![RenderAttachment {
                    clear: None,
                    ..attachment
                }],
                depth: None,
                resolves: vec![],
                size: [4, 4],
            });
            rec.push(Command::EndRendering);
        }
        rec.barriers([Barrier {
            resource: Resource::Image(image),
            before: if kind == 1 {
                Access::Depth
            } else {
                Access::Color
            },
            after: Access::CopySource,
            handoff: None,
        }]);
        rec.push(Command::ReadImage {
            source: image,
            destination: output,
        });
        rec.barriers([edge(
            Resource::Buffer(output),
            Access::CopyDestination,
            Access::HostRead,
        )]);
        let p = gpu.submit(rec.finish(), &[]).unwrap();
        gpu.wait(p).unwrap();
        let mut expected = vec![0; bytes as usize];
        if kind == 1 {
            for v in expected[656..672].as_chunks_mut::<4>().0 {
                v.copy_from_slice(&0.5f32.to_le_bytes());
            }
        } else {
            for v in expected[576..640].as_chunks_mut::<4>().0 {
                v.copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        assert_eq!(gpu.read(output).unwrap(), expected, "kind {kind}");
        gpu.assert_clean().unwrap();
        for (mip, layer) in [(3, 0), (0, 2)] {
            let mut bad = gpu.begin(Queue::Graphics);
            bad.push(Command::BeginRenderingSubresources {
                colors: if kind == 1 {
                    vec![]
                } else {
                    vec![RenderAttachment {
                        target: RenderTarget { image, mip, layer },
                        clear: Some([0.; 4]),
                    }]
                },
                depth: if kind == 1 {
                    Some(RenderAttachment {
                        target: RenderTarget { image, mip, layer },
                        clear: Some([0.; 4]),
                    })
                } else {
                    None
                },
                resolves: vec![],
                size: [1, 1],
            });
            bad.push(Command::EndRendering);
            assert!(gpu.submit(bad.finish(), &[]).is_err());
        }
        gpu.assert_clean().unwrap();
    }
}

/// A clear value and the texel bytes it leaves.
type Cleared = ([f32; 4], Vec<u8>);

/// Layer 0 of an array is a selected slice like any other: rendering to it must not
/// attach, or need a layout for, its sibling layers. Only the rendered layer is ever
/// transitioned here, colour and depth alike, so on Vulkan a view spanning both layers
/// fails validation (which runs in debug builds; the bytes alone would not show it).
#[test]
fn regression_first_layer_of_an_array_renders_without_touching_its_siblings() {
    use ara_gpu::{
        sync::{BarrierRange, ResourceBarrier},
        texture::ImageRegion,
    };
    let mut gpu = GpuDevice::new_raster(
        &[],
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-gpu/render-targets-first-layer"),
    )
    .unwrap();
    // The colour or depth each layer is cleared to, and the bytes that leaves.
    let cases: [(Format, Access, [Cleared; 2]); 2] = [
        (
            Format::Rgba8,
            Access::Color,
            [
                ([0.2, 0.4, 0.6, 1.0], vec![51, 102, 153, 255]),
                ([1.0, 0.8, 0.0, 1.0], vec![255, 204, 0, 255]),
            ],
        ),
        (
            Format::Depth32Float,
            Access::Depth,
            [
                ([0.25, 0.0, 0.0, 0.0], 0.25f32.to_le_bytes().to_vec()),
                ([0.75, 0.0, 0.0, 0.0], 0.75f32.to_le_bytes().to_vec()),
            ],
        ),
    ];
    for (format, attached, layers) in cases {
        let image = gpu
            .create_texture(&TextureDesc {
                shape: ImageShape {
                    layers: 2,
                    ..Default::default()
                },
                ..desc(format)
            })
            .unwrap();
        let layer = |layer, before, after| ResourceBarrier {
            barrier: edge(Resource::Image(image), before, after),
            range: BarrierRange::Image {
                base_mip: 0,
                mip_count: 1,
                base_layer: layer,
                layer_count: 1,
            },
        };
        // Layer 0 first and alone, each in its own submission as a renderer would.
        let mut reads = Vec::new();
        let mut last = None;
        for (index, (clear, _)) in layers.iter().enumerate() {
            let index = index as u32;
            let output = gpu.create_readback_buffer(8 * 8 * 4).unwrap();
            let target = RenderAttachment {
                target: RenderTarget {
                    image,
                    mip: 0,
                    layer: index,
                },
                clear: Some(*clear),
            };
            let depth = attached == Access::Depth;
            let mut rec = gpu.begin(Queue::Graphics);
            rec.barrier_ranges([layer(index, Access::Undefined, attached)]);
            rec.push(Command::BeginRenderingSubresources {
                colors: if depth { vec![] } else { vec![target] },
                depth: depth.then_some(target),
                resolves: vec![],
                size: [8, 8],
            });
            rec.push(Command::EndRendering);
            rec.barrier_ranges([layer(index, attached, Access::CopySource)]);
            rec.push(Command::ReadImageRegion {
                source: image,
                destination: output,
                buffer_offset: 0,
                region: ImageRegion {
                    mip: 0,
                    origin: [0; 3],
                    extent: [8, 8, 1],
                    base_layer: index,
                    layer_count: 1,
                },
            });
            rec.barriers([edge(
                Resource::Buffer(output),
                Access::CopyDestination,
                Access::HostRead,
            )]);
            last = Some(gpu.submit(rec.finish(), &[]).unwrap());
            reads.push(output);
        }
        gpu.wait(last.unwrap()).unwrap();
        for (read, (_, texel)) in reads.iter().zip(&layers) {
            assert_eq!(gpu.read(*read).unwrap(), texel.repeat(64), "{format:?}");
        }
        gpu.retire(Resource::Image(image)).unwrap();
        for buffer in reads {
            gpu.retire(Resource::Buffer(buffer)).unwrap();
        }
    }
    gpu.collect().unwrap();
    gpu.assert_clean().unwrap();
}
