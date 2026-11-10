//! Raster mip/slice selection preserves every other attachment subresource.
use super::*;
use crate::{
    commands::Command,
    descriptors::{Format, ImageDesc},
    rendering::{RenderAttachment, RenderTarget},
    texture::{ImageShape, SampleCount, TextureDesc},
};
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
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).unwrap();
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
        let mut entries = vec![(cooked::Stage::Vertex, "sliceVertex")];
        entries.push((
            cooked::Stage::Fragment,
            if kind == 1 {
                "sliceDepthFragment"
            } else {
                "sliceFragment"
            },
        ));
        let p = gpu.msl_pipeline_from(
            r#"
struct SliceVertex {float4 position [[position]];};
vertex SliceVertex sliceVertex(uint id [[vertex_id]]) {
    float2 uv=float2((id<<1)&2,id&2);
    return {float4(uv*2-1,0.5,1)};
}
fragment float4 sliceFragment() {return float4(1,0,0,1);}
fragment void sliceDepthFragment() {}
"#,
            &entries,
            &cooked::RenderState {
                color_formats: if kind == 1 {
                    vec![]
                } else {
                    vec![vk::Format::R8G8B8A8_UNORM.as_raw()]
                },
                depth_format: if kind == 1 {
                    Some(vk::Format::D32_SFLOAT.as_raw())
                } else {
                    None
                },
                depth_test: kind == 1,
                depth_write: kind == 1,
                raster: crate::raster::RasterState {
                    samples: if kind == 2 {
                        SampleCount::Four
                    } else {
                        SampleCount::One
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
        );
        rec.push(Command::Pipeline(p));
        rec.push(Command::Fullscreen);
        rec.push(Command::EndRendering);
        if kind == 0 {
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
    }
}
