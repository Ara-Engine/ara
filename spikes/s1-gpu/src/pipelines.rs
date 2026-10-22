use crate::{
    gpu::Gpu,
    scene::{Scene, Vertex},
};
use anyhow::{Context, Result};
use ash::vk;
use ash::vk::TaggedStructure;
use serde::Serialize;
use std::{io::Cursor, time::Instant};

/// Shared 32-byte push/root-constant ABI from the cooked Slang shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Constants {
    pub material: u32,
    pub texture: u32,
    pub buffer: u32,
    pub flags: u32,
    pub phase: f32,
    pub aspect: f32,
    pub padding: [u32; 2],
}

/// CPU timings and disk provenance for the pipeline cache experiment.
#[derive(Serialize)]
pub struct CacheMeasurements {
    pub previous_disk_bytes: usize,
    pub warm_seed: &'static str,
    pub cold_empty_cache_ms: [f64; 2],
    pub warm_disk_cache_ms: [f64; 2],
    pub persisted_bytes: usize,
    pub path: String,
    pub order: [&'static str; 2],
}

/// Global bindless layout, descriptor set and the two measured pipelines.
pub struct Pipelines {
    pub layout: vk::PipelineLayout,
    pub set: vk::DescriptorSet,
    pub graphics: vk::Pipeline,
    pub compute: vk::Pipeline,
    pub measurements: CacheMeasurements,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sampler: vk::Sampler,
}

impl Pipelines {
    /// Build the binding ABI, descriptors and empty/disk-seeded pipeline pairs.
    pub fn create(gpu: &Gpu, scene: &Scene, tlas: vk::AccelerationStructureKHR) -> Result<Self> {
        // SAFETY: Layout bindings match the offline shader ABI; all referenced
        // image/buffer/AS resources outlive these descriptors and pipelines.
        unsafe {
            let flags = vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::PARTIALLY_BOUND;
            let binding_flags = [
                flags,
                flags,
                vk::DescriptorBindingFlags::empty(),
                vk::DescriptorBindingFlags::empty(),
            ];
            let mut flags_info = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
                .binding_flags(&binding_flags);
            let all = vk::ShaderStageFlags::VERTEX
                | vk::ShaderStageFlags::FRAGMENT
                | vk::ShaderStageFlags::COMPUTE;
            let bindings = [
                vk::DescriptorSetLayoutBinding::default()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(16)
                    .stage_flags(all),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .descriptor_count(4)
                    .stage_flags(all),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(all),
                vk::DescriptorSetLayoutBinding::default()
                    .binding(3)
                    .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            ];
            let set_layout = gpu.device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default()
                    .bindings(&bindings)
                    .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
                    .push(&mut flags_info),
                None,
            )?;
            let sizes = bindings.map(|b| vk::DescriptorPoolSize {
                ty: b.descriptor_type,
                descriptor_count: b.descriptor_count,
            });
            let pool = gpu.device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .pool_sizes(&sizes)
                    .max_sets(1)
                    .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND),
                None,
            )?;
            let set = gpu.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&[set_layout]),
            )?[0];
            let sampler = gpu.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::REPEAT)
                    .address_mode_v(vk::SamplerAddressMode::REPEAT)
                    .address_mode_w(vk::SamplerAddressMode::REPEAT)
                    .max_lod(0.),
                None,
            )?;
            let image_infos: Vec<_> = scene
                .textures
                .iter()
                .enumerate()
                .map(|(index, i)| {
                    vk::DescriptorImageInfo::default()
                        // Slot 3 is replaced after the graphics command buffer binds
                        // this set, exercising UPDATE_AFTER_BIND rather than just flags.
                        .image_view(if index == 3 {
                            scene.textures[0].view
                        } else {
                            i.view
                        })
                        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                })
                .collect();
            let buffer_infos: Vec<_> = scene
                .material_buffers
                .iter()
                .map(|b| {
                    vk::DescriptorBufferInfo::default()
                        .buffer(b.handle)
                        .range(b.size)
                })
                .collect();
            let sampler_info = [vk::DescriptorImageInfo::default().sampler(sampler)];
            let structures = [tlas];
            let mut as_write = vk::WriteDescriptorSetAccelerationStructureKHR::default()
                .acceleration_structures(&structures);
            gpu.device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&image_infos),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                        .buffer_info(&buffer_infos),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(2)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&sampler_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(3)
                        .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                        .descriptor_count(1)
                        .push(&mut as_write),
                ],
                &[],
            );
            let constants = [vk::PushConstantRange::default()
                .offset(0)
                .size(std::mem::size_of::<Constants>() as u32)
                .stage_flags(all)];
            let layout = gpu.device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&[set_layout])
                    .push_constant_ranges(&constants),
                None,
            )?;
            let shaders = [
                module(gpu, "vertexMain")?,
                module(gpu, "fragmentMain")?,
                module(gpu, "computeMain")?,
            ];
            let key = gpu
                .info
                .pipeline_cache_uuid
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let path = format!("tools/pipeline-{key}-{}.bin", gpu.info.driver_version);
            let previous = std::fs::read(&path).unwrap_or_default();
            // Always measure an empty VkPipelineCache, then serialize and reload.
            // This does not flush NVIDIA's independent driver cache.
            let cold_cache = gpu
                .device
                .create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None)?;
            let (cold_graphics, cold_compute, cold_times) =
                create_pair(gpu, layout, cold_cache, &shaders)?;
            let bytes = gpu.device.get_pipeline_cache_data(cold_cache)?;
            std::fs::write(&path, &bytes)?;
            gpu.device.destroy_pipeline(cold_graphics, None);
            gpu.device.destroy_pipeline(cold_compute, None);
            gpu.device.destroy_pipeline_cache(cold_cache, None);
            let reloaded = std::fs::read(&path)?;
            let (seed, warm_seed) = if previous.is_empty() {
                (&reloaded, "serialized_in_this_process")
            } else {
                (&previous, "loaded_from_previous_process")
            };
            let warm_cache = gpu.device.create_pipeline_cache(
                &vk::PipelineCacheCreateInfo::default().initial_data(seed),
                None,
            )?;
            let (graphics, compute, warm_times) = create_pair(gpu, layout, warm_cache, &shaders)?;
            gpu.device.destroy_pipeline_cache(warm_cache, None);
            for shader in shaders {
                gpu.device.destroy_shader_module(shader, None);
            }
            Ok(Self {
                layout,
                set,
                graphics,
                compute,
                measurements: CacheMeasurements {
                    previous_disk_bytes: previous.len(),
                    warm_seed,
                    cold_empty_cache_ms: cold_times,
                    warm_disk_cache_ms: warm_times,
                    persisted_bytes: bytes.len(),
                    path,
                    order: ["graphics", "compute"],
                },
                set_layout,
                pool,
                sampler,
            })
        }
    }

    /// Release pipeline/layout objects after all submitted work is complete.
    pub fn destroy(&self, gpu: &Gpu) {
        // SAFETY: Called after all rendering has completed and before Gpu destruction.
        unsafe {
            gpu.device.destroy_pipeline(self.graphics, None);
            gpu.device.destroy_pipeline(self.compute, None);
            gpu.device.destroy_pipeline_layout(self.layout, None);
            gpu.device.destroy_descriptor_pool(self.pool, None);
            gpu.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            gpu.device.destroy_sampler(self.sampler, None);
        }
    }

    /// Replace a descriptor after command recording has bound the global heap.
    pub fn update_after_bind(&self, gpu: &Gpu, scene: &Scene) {
        let image = [vk::DescriptorImageInfo::default()
            .image_view(scene.textures[3].view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        // SAFETY: Binding 0 has UPDATE_AFTER_BIND, and the texture is already
        // uploaded. No submitted graphics work can read slot 3 before this write.
        unsafe {
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(0)
                    .dst_array_element(3)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&image)],
                &[],
            );
        }
    }
}

fn module(gpu: &Gpu, name: &str) -> Result<vk::ShaderModule> {
    let path = format!("cooked/{name}.spv");
    let bytes = std::fs::read(&path)
        .with_context(|| format!("Read {path}; run ./compile-shaders.ps1 first"))?;
    let words = ash::util::read_spv(&mut Cursor::new(bytes))?;
    // SAFETY: Word-aligned SPIR-V was compiled offline and validated by spirv-val.
    unsafe {
        Ok(gpu
            .device
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)?)
    }
}

fn create_pair(
    gpu: &Gpu,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    shaders: &[vk::ShaderModule; 3],
) -> Result<(vk::Pipeline, vk::Pipeline, [f64; 2])> {
    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(shaders[0])
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(shaders[1])
            .name(c"main"),
    ];
    let bindings = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<Vertex>() as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];
    let attributes = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32B32_SFLOAT,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32B32_SFLOAT,
            offset: 12,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 24,
        },
    ];
    let input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&bindings)
        .vertex_attribute_descriptions(&attributes);
    let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let raster = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.);
    let samples = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);
    let attachments = [vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
    let dynamic = vk::PipelineDynamicStateCreateInfo::default()
        .dynamic_states(&[vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR]);
    let formats = [vk::Format::R8G8B8A8_UNORM];
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&formats)
        .depth_attachment_format(vk::Format::D32_SFLOAT);
    let graphics_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&input)
        .input_assembly_state(&assembly)
        .viewport_state(&viewport)
        .rasterization_state(&raster)
        .multisample_state(&samples)
        .depth_stencil_state(&depth)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic)
        .layout(layout)
        .push(&mut rendering);
    let start = Instant::now();
    // SAFETY: All pipeline state pointers are live, layouts match the shader ABI,
    // and this function owns exclusive host access to the pipeline cache.
    let graphics = unsafe {
        gpu.device
            .create_graphics_pipelines(cache, &[graphics_info], None)
    }
    .map_err(|(_, e)| anyhow::anyhow!("graphics pipeline: {e:?}"))?[0];
    let graphics_ms = start.elapsed().as_secs_f64() * 1000.;
    let info = vk::ComputePipelineCreateInfo::default()
        .layout(layout)
        .stage(
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::COMPUTE)
                .module(shaders[2])
                .name(c"main"),
        );
    let start = Instant::now();
    // SAFETY: Same ownership/ABI guarantees as the graphics creation above.
    let compute = unsafe { gpu.device.create_compute_pipelines(cache, &[info], None) }
        .map_err(|(_, e)| anyhow::anyhow!("compute pipeline: {e:?}"))?[0];
    Ok((
        graphics,
        compute,
        [graphics_ms, start.elapsed().as_secs_f64() * 1000.],
    ))
}
