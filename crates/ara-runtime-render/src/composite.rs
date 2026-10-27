//! GPU-only landscape/scene depth composition; `Renderer::finish` grades the result.
use anyhow::Result;
use ara_gpu::{
    Access, Barrier, Device, GpuDevice, Image, Pipeline, Queue, Resource, RootConstants,
    commands::Command, vk,
};
use ara_render::graph::State;
use ara_vfx_gpu::{SceneColor, SceneDepth};
#[cfg(test)]
use std::path::Path;

pub struct Composite {
    pipeline: Pipeline,
    pub size: [u32; 2],
    hdr: Image,
    depth: Image,
    sampled: [u32; 2],
    storage: [u32; 2],
    access: Access,
}

fn barrier(image: Image, before: Access, after: Access) -> Barrier {
    Barrier {
        resource: Resource::Image(image),
        before,
        after,
        handoff: None,
    }
}

impl Composite {
    pub fn new(gpu: &mut GpuDevice, size: [u32; 2]) -> Result<Self> {
        let artifacts = ara_gpu::cooked::resource_dir(
            env!("CARGO_PKG_NAME"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
        );
        let cooked = serde_json::from_slice(&std::fs::read(artifacts.join("composite.json"))?)?;
        let pipeline = gpu.create_pipeline(&cooked, &artifacts)?;
        let usage = vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED;
        let hdr = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R32G32B32A32_SFLOAT,
            usage | vk::ImageUsageFlags::COLOR_ATTACHMENT,
        )?;
        let depth = gpu.create_image(size[0], size[1], vk::Format::R32G32B32A32_SFLOAT, usage)?;
        Ok(Self {
            pipeline,
            size,
            hdr,
            depth,
            sampled: [
                gpu.sampled_image(hdr, false)?.index,
                gpu.sampled_image(depth, false)?.index,
            ],
            storage: [
                gpu.storage_image(hdr)?.index,
                gpu.storage_image(depth)?.index,
            ],
            access: Access::Undefined,
        })
    }

    pub fn merge(
        &mut self,
        gpu: &mut GpuDevice,
        scene: SceneColor,
        depth: SceneDepth,
        landscape: (Image, Image),
        exposure: f32,
    ) -> Result<(SceneColor, SceneDepth)> {
        let (color, land_depth) = landscape;
        let sampled = [
            gpu.sampled_image(color, false)?.index,
            gpu.sampled_image(land_depth, true)?.index,
        ];
        let mut r = gpu.begin(Queue::Graphics);
        r.barriers([
            barrier(color, Access::Color, Access::ShaderRead),
            barrier(land_depth, Access::Depth, Access::ShaderRead),
            barrier(self.hdr, self.access, Access::Storage),
            barrier(self.depth, self.access, Access::Storage),
        ]);
        r.push(Command::Pipeline(self.pipeline));
        r.push(Command::Constants(RootConstants {
            words: [
                scene.sampled_index,
                depth.sampled_index,
                sampled[0],
                sampled[1],
                self.storage[0],
                self.storage[1],
                exposure.to_bits(),
                0,
            ],
        }));
        r.push(Command::Dispatch([
            self.size[0].div_ceil(8),
            self.size[1].div_ceil(8),
            1,
        ]));
        r.barriers([
            barrier(self.hdr, Access::Storage, Access::ShaderRead),
            barrier(self.depth, Access::Storage, Access::ShaderRead),
            barrier(color, Access::ShaderRead, Access::Color),
            barrier(land_depth, Access::ShaderRead, Access::Depth),
        ]);
        let point = gpu.submit(r.finish(), &[])?;
        gpu.resolve(point)?;
        self.access = Access::ShaderRead;
        let state = State {
            access: Access::ShaderRead,
            queue: Queue::Graphics,
        };
        Ok((
            SceneColor {
                image: self.hdr,
                sampled_index: self.sampled[0],
                state,
            },
            SceneDepth {
                image: self.depth,
                sampled_index: self.sampled[1],
                state,
            },
        ))
    }

    pub fn retire(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire_pipeline(self.pipeline)?;
        for image in [self.hdr, self.depth] {
            gpu.retire(Resource::Image(image))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn landscape_display_color_survives_hdr_composition() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/runtime-polish/composite-test");
        let mut gpu = GpuDevice::new(&[], root)?;
        let size = [8, 8];
        let mut upload = |format, bytes: &[u8], after| -> Result<(Image, u32)> {
            let usage = vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_DST
                | if after == Access::Color {
                    vk::ImageUsageFlags::COLOR_ATTACHMENT
                } else if after == Access::Depth {
                    vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                } else {
                    vk::ImageUsageFlags::empty()
                };
            let image = gpu.create_image(8, 8, format, usage)?;
            let staging =
                gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
            gpu.upload(staging, bytes)?;
            let mut r = gpu.begin(Queue::Graphics);
            r.barriers([
                barrier(image, Access::Undefined, Access::CopyDestination),
                Barrier {
                    resource: Resource::Buffer(staging),
                    before: Access::HostWrite,
                    after: Access::CopySource,
                    handoff: None,
                },
            ]);
            r.push(Command::UploadImage {
                source: staging,
                destination: image,
            });
            r.barriers([barrier(image, Access::CopyDestination, after)]);
            let p = gpu.submit(r.finish(), &[])?;
            gpu.resolve(p)?;
            gpu.retire(Resource::Buffer(staging))?;
            let sampled = gpu
                .sampled_image(image, format == vk::Format::D32_SFLOAT)?
                .index;
            Ok((image, sampled))
        };
        let (scene, scene_index) = upload(
            vk::Format::R32G32B32A32_SFLOAT,
            &vec![0; 8 * 8 * 16],
            Access::ShaderRead,
        )?;
        let (depth, depth_index) = upload(
            vk::Format::R32G32B32A32_SFLOAT,
            &vec![0; 8 * 8 * 16],
            Access::ShaderRead,
        )?;
        let (color, _) = upload(
            vk::Format::R8G8B8A8_UNORM,
            &vec![128; 8 * 8 * 4],
            Access::Color,
        )?;
        let values: Vec<u8> = (0..64).flat_map(|_| 1.0f32.to_le_bytes()).collect();
        let (land_depth, _) = upload(vk::Format::D32_SFLOAT, &values, Access::Depth)?;
        let state = State {
            access: Access::ShaderRead,
            queue: Queue::Graphics,
        };
        let mut composite = Composite::new(&mut gpu, size)?;
        // The default raster look grades exactly like the shared baseline tone map.
        let post = ara_render::renderer::post::ToneMap::new(&mut gpu)?;
        let image = gpu.create_image(
            8,
            8,
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let output = gpu.storage_image(image)?.index;
        let mut image_access = Access::Undefined;
        for exposure in [1.0, 0.0022] {
            let (hdr, _) = composite.merge(
                &mut gpu,
                SceneColor {
                    image: scene,
                    sampled_index: scene_index,
                    state,
                },
                SceneDepth {
                    image: depth,
                    sampled_index: depth_index,
                    state,
                },
                (color, land_depth),
                exposure,
            )?;
            post.prepare(&gpu, size, exposure, output, true)?;
            let read = gpu.create_buffer(8 * 8 * 4, vk::BufferUsageFlags::TRANSFER_DST, true)?;
            let mut r = gpu.begin(Queue::Graphics);
            r.barriers([
                barrier(image, image_access, Access::Storage),
                Barrier {
                    resource: Resource::Buffer(post.frame_buffer()),
                    before: Access::HostWrite,
                    after: Access::ShaderRead,
                    handoff: None,
                },
            ]);
            post.record(&mut r, hdr.sampled_index, size);
            r.barriers([barrier(image, Access::Storage, Access::CopySource)]);
            image_access = Access::CopySource;
            r.push(Command::ReadImage {
                source: image,
                destination: read,
            });
            r.barriers([Barrier {
                resource: Resource::Buffer(read),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            }]);
            let p = gpu.submit(r.finish(), &[])?;
            gpu.resolve(p)?;
            let pixels = gpu.read(read)?;
            let first = pixels.first().context("readback pixel")?;
            assert!(
                (110..=145).contains(first),
                "landscape gamma/HDR round trip at exposure {exposure}: {pixels:?}"
            );
            gpu.retire(Resource::Buffer(read))?;
        }
        gpu.assert_clean()?;
        Ok(())
    }
}
