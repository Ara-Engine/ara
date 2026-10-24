use super::Context;
use crate::{
    GpuError, Result,
    cooked::{CookedPipeline, Stage, binding_layout_hash, legacy_preview_layout_hash},
};
use ash::vk::{self, TaggedStructure};
use sha2::{Digest, Sha256};
use std::{io::Cursor, path::Path};

pub(super) struct NativePipeline {
    pub handle: vk::Pipeline,
    pub compute: bool,
    pub mesh: bool,
    pub colors: Vec<vk::Format>,
    pub depth: Option<vk::Format>,
    pub vertex_stride: u32,
}

pub(super) fn create(
    gpu: &Context,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    cooked: &CookedPipeline,
    dir: &Path,
) -> Result<NativePipeline> {
    create_on_device(
        &gpu.device,
        gpu.info.mesh_shading,
        layout,
        cache,
        cooked,
        dir,
    )
}

/// The caller retains device/layout ownership until compilation finishes; cache access is exclusive.
pub(super) fn create_on_device(
    device: &ash::Device,
    mesh_shading: bool,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    cooked: &CookedPipeline,
    dir: &Path,
) -> Result<NativePipeline> {
    let legacy_preview = cooked.stages.len() == 2
        && cooked.stages.iter().all(|stage| {
            matches!(
                (stage.stage, stage.source_entry.as_str()),
                (Stage::Vertex, "previewVertex") | (Stage::Fragment, "previewFragment")
            )
        });
    if cooked.version != 1
        || (cooked.binding_layout_hash != binding_layout_hash()
            && !(legacy_preview && cooked.binding_layout_hash == legacy_preview_layout_hash()))
    {
        return Err(GpuError::Contract(
            "Cooked layout/schema mismatch; recook shaders".into(),
        ));
    }
    let mesh = cooked.stages.iter().any(|s| s.stage == Stage::Mesh);
    let task = cooked.stages.iter().any(|s| s.stage == Stage::Task);
    if (mesh || task)
        && (!mesh_shading
            || !mesh
            || cooked
                .stages
                .iter()
                .any(|s| matches!(s.stage, Stage::Vertex | Stage::Compute))
            || cooked.state.vertex_stride != 0)
    {
        return Err(GpuError::Contract(
            "Mesh/task pipeline unavailable or incompatible stages".into(),
        ));
    }
    let mut modules = Vec::new();
    let result = (|| {
        for stage in &cooked.stages {
            let bytes = std::fs::read(dir.join(&stage.spirv)).map_err(anyhow::Error::from)?;
            if format!("{:x}", Sha256::digest(&bytes)) != stage.spirv_hash {
                return Err(GpuError::Contract(format!(
                    "Shader content hash mismatch: {}",
                    stage.spirv
                )));
            }
            let words =
                ash::util::read_spv(&mut Cursor::new(bytes)).map_err(anyhow::Error::from)?;
            // SAFETY: Offline SPIR-V has a checked content hash and correct word alignment.
            modules.push(
                unsafe {
                    device.create_shader_module(
                        &vk::ShaderModuleCreateInfo::default().code(&words),
                        None,
                    )
                }
                .map_err(anyhow::Error::from)?,
            );
        }
        let stages = cooked
            .stages
            .iter()
            .zip(&modules)
            .map(|(s, m)| {
                if s.spirv_entry != "main" {
                    return Err(GpuError::Contract("Unsupported SPIR-V entry ABI".into()));
                }
                Ok(vk::PipelineShaderStageCreateInfo::default()
                    .module(*m)
                    .name(c"main")
                    .stage(match s.stage {
                        Stage::Vertex => vk::ShaderStageFlags::VERTEX,
                        Stage::Fragment => vk::ShaderStageFlags::FRAGMENT,
                        Stage::Compute => vk::ShaderStageFlags::COMPUTE,
                        Stage::Task => vk::ShaderStageFlags::TASK_EXT,
                        Stage::Mesh => vk::ShaderStageFlags::MESH_EXT,
                    }))
            })
            .collect::<Result<Vec<_>>>()?;
        let compute = cooked.stages.len() == 1 && cooked.stages[0].stage == Stage::Compute;
        // SAFETY: State and shader modules remain live through creation; host cache access is exclusive.
        let handle = unsafe {
            if compute {
                device.create_compute_pipelines(
                    cache,
                    &[vk::ComputePipelineCreateInfo::default()
                        .layout(layout)
                        .stage(stages[0])],
                    None,
                )
            } else {
                let state = &cooked.state;
                let bindings = [vk::VertexInputBindingDescription {
                    binding: 0,
                    stride: state.vertex_stride,
                    input_rate: vk::VertexInputRate::VERTEX,
                }];
                let attributes = [
                    (0, vk::Format::R32G32B32_SFLOAT, 0),
                    (1, vk::Format::R32G32B32_SFLOAT, 12),
                    (2, vk::Format::R32G32_SFLOAT, 24),
                    (3, vk::Format::R32G32B32A32_SFLOAT, 32),
                ]
                .map(|(location, format, offset)| {
                    vk::VertexInputAttributeDescription {
                        location,
                        binding: 0,
                        format,
                        offset,
                    }
                });
                let mut input = vk::PipelineVertexInputStateCreateInfo::default();
                if state.vertex_stride != 0 {
                    input = input
                        .vertex_binding_descriptions(&bindings)
                        .vertex_attribute_descriptions(&attributes);
                }
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
                    .depth_test_enable(state.depth_test)
                    .depth_write_enable(state.depth_write)
                    .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL);
                let attachments = state
                    .color_formats
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        vk::PipelineColorBlendAttachmentState::default()
                            .color_write_mask(vk::ColorComponentFlags::RGBA)
                            .blend_enable(state.blend && i == 0)
                            .src_color_blend_factor(vk::BlendFactor::ONE)
                            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                            .color_blend_op(vk::BlendOp::ADD)
                            .src_alpha_blend_factor(vk::BlendFactor::ONE)
                            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                            .alpha_blend_op(vk::BlendOp::ADD)
                    })
                    .collect::<Vec<_>>();
                let blend =
                    vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
                let dynamic = vk::PipelineDynamicStateCreateInfo::default()
                    .dynamic_states(&[vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR]);
                let formats = state
                    .color_formats
                    .iter()
                    .map(|f| vk::Format::from_raw(*f))
                    .collect::<Vec<_>>();
                let mut rendering = vk::PipelineRenderingCreateInfo::default()
                    .color_attachment_formats(&formats)
                    .depth_attachment_format(
                        state
                            .depth_format
                            .map(vk::Format::from_raw)
                            .unwrap_or(vk::Format::UNDEFINED),
                    );
                let mut info = vk::GraphicsPipelineCreateInfo::default()
                    .stages(&stages)
                    .viewport_state(&viewport)
                    .rasterization_state(&raster)
                    .multisample_state(&samples)
                    .depth_stencil_state(&depth)
                    .color_blend_state(&blend)
                    .dynamic_state(&dynamic)
                    .layout(layout)
                    .push(&mut rendering);
                if !mesh {
                    info = info
                        .vertex_input_state(&input)
                        .input_assembly_state(&assembly);
                }
                device.create_graphics_pipelines(cache, &[info], None)
            }
        }
        .map_err(|(partial, error)| {
            // SAFETY: Partially created pipelines have not been submitted.
            unsafe {
                for pipeline in partial {
                    device.destroy_pipeline(pipeline, None);
                }
            }
            GpuError::Backend(anyhow::anyhow!("Pipeline creation: {error:?}"))
        })?[0];
        Ok(NativePipeline {
            handle,
            compute,
            mesh,
            colors: cooked
                .state
                .color_formats
                .iter()
                .map(|f| vk::Format::from_raw(*f))
                .collect(),
            depth: cooked.state.depth_format.map(vk::Format::from_raw),
            vertex_stride: cooked.state.vertex_stride,
        })
    })();
    // SAFETY: Pipelines retain compiled shader state; modules are not needed after creation.
    unsafe {
        for module in modules {
            device.destroy_shader_module(module, None);
        }
    }
    result
}
