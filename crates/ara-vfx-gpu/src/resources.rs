use crate::{
    COMPUTE_ENTRIES,
    layout::*,
    runtime::{GpuSystem, Pipelines, Tracked},
};
use anyhow::{Result, ensure};
use ara_gpu::{Access, Buffer, Device, Image, Pipeline, Queue, Resource, vk, vulkan::VulkanDevice};
use ara_render::{capture::ShaderMetadata, graph::State};
use ara_vfx::Effect;
use bytemuck::Zeroable;
use std::path::Path;

impl GpuSystem {
    /// Load matching offline pipelines and allocate device-local SoA/list storage.
    /// No shader compiler runs here. Shader/pipeline publication belongs at a frame boundary.
    pub fn new(
        gpu: &mut VulkanDevice,
        effect: &Effect,
        cooked: &Path,
        size: [u32; 2],
    ) -> Result<Self> {
        ensure!(!size.contains(&0), "zero VFX capture extent");
        let hash = effect
            .content_hash()
            .map_err(|d| anyhow::anyhow!("{d:?}"))?;
        let effect = effect
            .resolve(&Default::default())
            .map_err(|d| anyhow::anyhow!("{d:?}"))?;
        let stored: serde_json::Value =
            ara_json::parse(&std::fs::read_to_string(cooked.join("effect.json"))?)?;
        ensure!(
            stored.get("hash").and_then(|h| h.as_str()) == Some(hash.as_str()),
            "cooked VFX effect hash differs; recook in a worker before publication"
        );
        let mut cohort = None;
        let mut load = |name: &str| -> Result<Pipeline> {
            let p: ara_gpu::cooked::CookedPipeline =
                serde_json::from_slice(&std::fs::read(cooked.join(format!("{name}.json")))?)?;
            if let Some(ref old) = cohort {
                ensure!(*old == p.key, "mixed VFX shader cohort");
            } else {
                cohort = Some(p.key.clone());
            }
            Ok(gpu.create_pipeline(&p, cooked)?)
        };
        let compute = COMPUTE_ENTRIES
            .iter()
            .map(|name| load(name))
            .collect::<Result<Vec<_>>>()?;
        let pipelines = Pipelines {
            compute,
            backdrop: load("backdrop")?,
            particles: load("particles")?,
            tonemap: load("tonemap")?,
        };
        let p: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(cooked.join("particles.json"))?)?;
        let metadata = ShaderMetadata {
            content_key: p.key,
            compiler: p.compiler,
            binding_layout_hash: p.binding_layout_hash,
        };
        let layout = Layout::new(&effect)?;
        let mut resources = Vec::new();
        let mut buffer = |gpu: &mut VulkanDevice,
                          size: u64,
                          host: bool,
                          usage: vk::BufferUsageFlags|
         -> Result<Buffer> {
            let buffer = gpu.create_buffer(size, usage, host)?;
            resources.push(Tracked {
                resource: Resource::Buffer(buffer),
                state: State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                },
            });
            Ok(buffer)
        };
        let storage = vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::TRANSFER_DST
            | vk::BufferUsageFlags::TRANSFER_SRC;
        let particles = buffer(gpu, u64::from(layout.total) * 64, false, storage)?;
        let lists = buffer(gpu, u64::from(layout.list_bytes), false, storage)?;
        let control = buffer(
            gpu,
            layout.configs.len() as u64 * CONTROL_STRIDE as u64,
            false,
            storage | vk::BufferUsageFlags::INDIRECT_BUFFER,
        )?;
        let config = buffer(
            gpu,
            layout.configs.len() as u64 * CONFIG_STRIDE as u64,
            true,
            storage,
        )?;
        let frame = buffer(gpu, std::mem::size_of::<Frame>() as u64, true, storage)?;
        let read_control = buffer(
            gpu,
            layout.configs.len() as u64 * CONTROL_STRIDE as u64,
            true,
            vk::BufferUsageFlags::TRANSFER_DST,
        )?;
        let read_color = buffer(
            gpu,
            u64::from(size[0]) * u64::from(size[1]) * 4,
            true,
            vk::BufferUsageFlags::TRANSFER_DST,
        )?;
        let mut image = |format, usage| -> Result<Image> {
            let i = gpu.create_image(size[0], size[1], format, usage)?;
            resources.push(Tracked {
                resource: Resource::Image(i),
                state: State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                },
            });
            Ok(i)
        };
        let attachment = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
        let hdr = image(vk::Format::R32G32B32A32_SFLOAT, attachment)?;
        let depth = image(vk::Format::R32_SFLOAT, attachment)?;
        let output = image(
            vk::Format::R8G8B8A8_UNORM,
            attachment | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let feature_ids = image(
            vk::Format::R32_SFLOAT,
            attachment | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let read_ids = gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 4,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        resources.push(Tracked {
            resource: Resource::Buffer(read_ids),
            state: State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            },
        });
        let material_buffer =
            gpu.create_buffer(16 * 64 * 16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        gpu.upload(material_buffer, &vec![0u8; 16 * 64 * 16])?;
        resources.push(Tracked {
            resource: Resource::Buffer(material_buffer),
            state: State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        });
        let material_index = gpu.storage_buffer(material_buffer)?.index;
        let indices = [
            gpu.storage_buffer(particles)?.index,
            gpu.storage_buffer(lists)?.index,
            gpu.storage_buffer(control)?.index,
            gpu.storage_buffer(frame)?.index,
        ];
        let config_index = gpu.storage_buffer(config)?.index;
        let depth_index = gpu.sampled_image(depth, false)?.index;
        let hdr_index = gpu.sampled_image(hdr, false)?.index;
        let atlas_bytes=std::fs::read(cooked.join("../atlas.rgba"))?;
        let atlas=crate::atlas::upload(gpu,&atlas_bytes)?;
        let atlas_index=gpu.sampled_image(atlas,false)?.index;
        resources.push(Tracked {resource:Resource::Image(atlas),state:State {access:Access::ShaderRead,queue:Queue::Graphics}});
        gpu.upload(config, bytemuck::cast_slice(&layout.configs))?;
        resources[3].state.access = Access::HostWrite;
        let mut result = Self {
            effect,
            layout,
            resources,
            particles,
            lists,
            control,
            frame,
            read_control,
            read_color,
            hdr,
            depth,
            output,
            feature_ids,
            read_ids,
            material_buffer,
            material_index,
            material_textures: Vec::new(),
            indices,
            config_index,
            depth_index,
            hdr_index,
            atlas_index,
            color_target: hdr,
            color_index: hdr_index,
            preview_background: true,
            pipelines,
            size,
            tick: 0,
            timings: vec![],
            metadata,
            hash,
            visible: 0,
            async_compute: false,
            last_frame: Frame::zeroed(),
        };
        result.reset(gpu)?;
        Ok(result)
    }
    /// Reset deterministic replay state without changing compiled programs.
    pub fn reset(&mut self, gpu: &mut VulkanDevice) -> Result<()> {
        let mut particle_data = vec![0u32; self.layout.total as usize * 16];
        for slot in 0..self.layout.total as usize {
            particle_data[slot * 4 + 3] = (-1f32).to_bits();
        }
        let mut lists = vec![0u32; self.layout.list_bytes as usize / 4];
        let mut control = vec![0u32; self.layout.configs.len() * 16];
        for (i, c) in self.layout.configs.iter().enumerate() {
            for slot in 0..c.partition[1] {
                lists[(c.lists[1] / 4 + slot) as usize] = slot;
            }
            control[i * 16 + 1] = c.partition[1];
            control[i * 16 + 8] = c.partition[1].div_ceil(256);
            control[i * 16 + 9] = 1;
            control[i * 16 + 10] = 1;
        }
        self.initialize_buffer(gpu, 0, bytemuck::cast_slice(&particle_data))?;
        self.initialize_buffer(gpu, 1, bytemuck::cast_slice(&lists))?;
        self.initialize_buffer(gpu, 2, bytemuck::cast_slice(&control))?;
        self.tick = 0;
        self.timings.clear();
        Ok(())
    }
    fn initialize_buffer(
        &mut self,
        gpu: &mut VulkanDevice,
        index: usize,
        bytes: &[u8],
    ) -> Result<()> {
        use ara_gpu::{Barrier, commands::Command};
        let Resource::Buffer(destination) = self.resources[index].resource else {
            anyhow::bail!("initialization expects buffer");
        };
        let staging =
            gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
        gpu.upload(staging, bytes)?;
        let state = self.resources[index].state;
        let mut r = gpu.begin(state.queue);
        r.push(Command::Barrier(Barrier {
            resource: Resource::Buffer(staging),
            before: Access::HostWrite,
            after: Access::CopySource,
            handoff: None,
        }));
        r.push(Command::Barrier(Barrier {
            resource: Resource::Buffer(destination),
            before: state.access,
            after: Access::CopyDestination,
            handoff: None,
        }));
        r.push(Command::CopyBuffer {
            source: staging,
            destination,
            size: bytes.len() as u64,
        });
        let point = gpu.submit(r.finish(), &[])?;
        gpu.resolve(point)?;
        gpu.retire(Resource::Buffer(staging))?;
        gpu.collect()?;
        self.resources[index].state.access = Access::CopyDestination;
        Ok(())
    }
}
