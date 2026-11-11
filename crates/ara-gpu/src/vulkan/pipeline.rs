use super::Context;
use crate::{
    GpuError, Result,
    cooked::{CookedPipeline, Stage, binding_layout_compatible, legacy_preview_layout_hash},
};
use ash::vk::{self, TaggedStructure};
use sha2::{Digest, Sha256};
use std::{io::Cursor, path::Path};

/// Optional device features a cooked pipeline's stages may need.
#[derive(Clone, Copy)]
pub(super) struct ShaderFeatures {
    /// Mesh and task stages (`VK_EXT_mesh_shader`).
    pub mesh_shading: bool,
    /// `FragmentBarycentricKHR` in a fragment stage.
    pub fragment_barycentrics: bool,
    /// `SampledCubeArray` in any stage.
    pub cube_arrays: bool,
}
impl ShaderFeatures {
    pub(super) fn of(info: &super::DeviceInfo) -> Self {
        Self {
            mesh_shading: info.mesh_shading,
            fragment_barycentrics: info.fragment_barycentrics,
            cube_arrays: info.cube_arrays,
        }
    }
}
/// `OpCapability FragmentBarycentricKHR`.
const FRAGMENT_BARYCENTRIC: u32 = 5284;
/// `OpCapability SampledCubeArray`.
const SAMPLED_CUBE_ARRAY: u32 = 45;
/// Whether a SPIR-V module declares `capability`. Capabilities lead the module, right
/// after its five-word header, so the scan stops at the first other instruction.
fn declares_capability(words: &[u32], capability: u32) -> bool {
    let mut at = 5;
    while let Some(&word) = words.get(at) {
        let (count, opcode) = ((word >> 16) as usize, word & 0xffff);
        if opcode != 17 || count != 2 {
            return false;
        }
        if words.get(at + 1) == Some(&capability) {
            return true;
        }
        at += count;
    }
    false
}

pub(super) struct NativePipeline {
    pub handle: vk::Pipeline,
    pub compute: bool,
    pub mesh: bool,
    pub colors: Vec<vk::Format>,
    pub depth: Option<vk::Format>,
    pub vertex_stride: u32,
    pub raster: crate::raster::RasterState,
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
        ShaderFeatures::of(&gpu.info),
        layout,
        cache,
        cooked,
        dir,
    )
}

/// The caller retains device/layout ownership until compilation finishes; cache access is exclusive.
pub(super) fn create_on_device(
    device: &ash::Device,
    features: ShaderFeatures,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    cooked: &CookedPipeline,
    dir: &Path,
) -> Result<NativePipeline> {
    create_inner(device, features, layout, cache, cooked, dir, None)
}

#[derive(Default)]
pub(super) struct CreationTimes {
    pub load_ms: f64,
    pub pso_ms: f64,
}

pub(super) fn create_profiled(
    device: &ash::Device,
    features: ShaderFeatures,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    cooked: &CookedPipeline,
    dir: &Path,
    times: &mut CreationTimes,
) -> Result<NativePipeline> {
    create_inner(device, features, layout, cache, cooked, dir, Some(times))
}

pub(super) fn create_inner(
    device: &ash::Device,
    features: ShaderFeatures,
    layout: vk::PipelineLayout,
    cache: vk::PipelineCache,
    cooked: &CookedPipeline,
    dir: &Path,
    mut times: Option<&mut CreationTimes>,
) -> Result<NativePipeline> {
    let trace_started = std::time::Instant::now();
    let legacy_preview = cooked.stages.len() == 2
        && cooked.stages.iter().all(|stage| {
            matches!(
                (stage.stage, stage.source_entry.as_str()),
                (Stage::Vertex, "previewVertex") | (Stage::Fragment, "previewFragment")
            )
        });
    cooked.state.validate()?;
    if cooked.version != 1
        || (!binding_layout_compatible(&cooked.binding_layout_hash)
            && !(legacy_preview && cooked.binding_layout_hash == legacy_preview_layout_hash()))
    {
        return Err(GpuError::Contract(
            "Cooked layout/schema mismatch; recook shaders".into(),
        ));
    }
    let mesh = cooked.stages.iter().any(|s| s.stage == Stage::Mesh);
    let task = cooked.stages.iter().any(|s| s.stage == Stage::Task);
    if (mesh || task)
        && (!features.mesh_shading
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
            if !features.fragment_barycentrics && declares_capability(&words, FRAGMENT_BARYCENTRIC)
            {
                return Err(GpuError::Contract(format!(
                    "{} reads fragment barycentrics, which this device does not support",
                    stage.spirv
                )));
            }
            if !features.cube_arrays && declares_capability(&words, SAMPLED_CUBE_ARRAY) {
                return Err(GpuError::Contract(format!(
                    "{} samples cube arrays, which this device does not support",
                    stage.spirv
                )));
            }
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
        let pso_started = times.as_ref().map(|_| std::time::Instant::now());
        if let Some(times) = &mut times {
            times.load_ms += trace_started.elapsed().as_secs_f64() * 1000.;
        }
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
                let explicit = &state.raster.vertex_layouts;
                let bindings: Vec<_> = if explicit.is_empty() {
                    vec![vk::VertexInputBindingDescription {
                        binding: 0,
                        stride: state.vertex_stride,
                        input_rate: vk::VertexInputRate::VERTEX,
                    }]
                } else {
                    explicit
                        .iter()
                        .enumerate()
                        .map(|(i, b)| vk::VertexInputBindingDescription {
                            binding: i as u32,
                            stride: b.stride,
                            input_rate: if b.step == crate::raster::VertexStep::Instance {
                                vk::VertexInputRate::INSTANCE
                            } else {
                                vk::VertexInputRate::VERTEX
                            },
                        })
                        .collect()
                };
                let attributes: Vec<_> = if explicit.is_empty() {
                    [
                        (0, vk::Format::R32G32B32_SFLOAT, 0),
                        (1, vk::Format::R32G32B32_SFLOAT, 12),
                        (2, vk::Format::R32G32_SFLOAT, 24),
                        (3, vk::Format::R32G32B32A32_SFLOAT, 32),
                    ]
                    .map(
                        |(location, format, offset)| vk::VertexInputAttributeDescription {
                            location,
                            binding: 0,
                            format,
                            offset,
                        },
                    )
                    .to_vec()
                } else {
                    explicit
                        .iter()
                        .enumerate()
                        .flat_map(|(i, b)| {
                            b.attributes
                                .iter()
                                .map(move |a| vk::VertexInputAttributeDescription {
                                    location: a.location,
                                    binding: i as u32,
                                    format: a.format.vk(),
                                    offset: a.offset,
                                })
                        })
                        .collect()
                };
                let mut input = vk::PipelineVertexInputStateCreateInfo::default();
                if state.vertex_stride != 0 || !explicit.is_empty() {
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
                    .cull_mode(match state.raster.cull {
                        crate::raster::CullMode::None => vk::CullModeFlags::NONE,
                        crate::raster::CullMode::Front => vk::CullModeFlags::FRONT,
                        crate::raster::CullMode::Back => vk::CullModeFlags::BACK,
                    })
                    .front_face(
                        if state.raster.front_face == crate::raster::FrontFace::Clockwise {
                            vk::FrontFace::CLOCKWISE
                        } else {
                            vk::FrontFace::COUNTER_CLOCKWISE
                        },
                    )
                    .depth_bias_enable(
                        state.raster.depth_bias != 0.0 || state.raster.depth_bias_slope != 0.0,
                    )
                    .depth_bias_constant_factor(state.raster.depth_bias)
                    .depth_bias_slope_factor(state.raster.depth_bias_slope)
                    .line_width(1.);
                let samples = vk::PipelineMultisampleStateCreateInfo::default()
                    .rasterization_samples(state.raster.samples.vk());
                let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                    .depth_test_enable(state.depth_test)
                    .depth_write_enable(state.depth_write)
                    .depth_compare_op(state.raster.depth_compare.vk());
                let attachments = state
                    .color_formats
                    .iter()
                    .enumerate()
                    .map(|(i, _)| {
                        let base = vk::PipelineColorBlendAttachmentState::default()
                            .color_write_mask(vk::ColorComponentFlags::RGBA)
                            .blend_enable(state.blend && i == 0)
                            .src_color_blend_factor(vk::BlendFactor::ONE)
                            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                            .color_blend_op(vk::BlendOp::ADD)
                            .src_alpha_blend_factor(vk::BlendFactor::ONE)
                            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                            .alpha_blend_op(vk::BlendOp::ADD);
                        match state.raster.blends.get(i) {
                            Some(Some(b)) => base
                                .blend_enable(true)
                                .src_color_blend_factor(blend_factor(b.color.source))
                                .dst_color_blend_factor(blend_factor(b.color.destination))
                                .color_blend_op(blend_op(b.color.operation))
                                .src_alpha_blend_factor(blend_factor(b.alpha.source))
                                .dst_alpha_blend_factor(blend_factor(b.alpha.destination))
                                .alpha_blend_op(blend_op(b.alpha.operation)),
                            Some(None) => base.blend_enable(false),
                            None => base,
                        }
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
        if let (Some(times), Some(started)) = (&mut times, pso_started) {
            times.pso_ms += started.elapsed().as_secs_f64() * 1000.;
        }
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
            raster: cooked.state.raster.clone(),
        })
    })();
    // SAFETY: Pipelines retain compiled shader state; modules are not needed after creation.
    unsafe {
        for module in modules {
            device.destroy_shader_module(module, None);
        }
    }
    if std::env::var_os("ARA_PIPELINE_TRACE").is_some() {
        eprintln!(
            "PIPELINE_TRACE thread={:?} entries={:?} ms={:.3}",
            std::thread::current().name(),
            cooked
                .stages
                .iter()
                .map(|s| s.source_entry.as_str())
                .collect::<Vec<_>>(),
            trace_started.elapsed().as_secs_f64() * 1000.
        );
    }
    result
}

fn blend_factor(f: crate::raster::BlendFactor) -> vk::BlendFactor {
    use crate::raster::BlendFactor as F;
    match f {
        F::Zero => vk::BlendFactor::ZERO,
        F::One => vk::BlendFactor::ONE,
        F::SrcColor => vk::BlendFactor::SRC_COLOR,
        F::OneMinusSrcColor => vk::BlendFactor::ONE_MINUS_SRC_COLOR,
        F::DstColor => vk::BlendFactor::DST_COLOR,
        F::OneMinusDstColor => vk::BlendFactor::ONE_MINUS_DST_COLOR,
        F::SrcAlpha => vk::BlendFactor::SRC_ALPHA,
        F::OneMinusSrcAlpha => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        F::DstAlpha => vk::BlendFactor::DST_ALPHA,
        F::OneMinusDstAlpha => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
    }
}
fn blend_op(o: crate::raster::BlendOp) -> vk::BlendOp {
    use crate::raster::BlendOp as O;
    match o {
        O::Add => vk::BlendOp::ADD,
        O::Subtract => vk::BlendOp::SUBTRACT,
        O::ReverseSubtract => vk::BlendOp::REVERSE_SUBTRACT,
        O::Min => vk::BlendOp::MIN,
        O::Max => vk::BlendOp::MAX,
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    #[test]
    fn capability_scan_reads_only_the_leading_capabilities() {
        let header = [0x0723_0203, 0x0001_0600, 0, 8, 0];
        let capability = |c| [(2 << 16) | 17, c];
        // Shader, then FragmentBarycentricKHR, then OpExtension "x".
        let mut module = header.to_vec();
        module.extend(capability(1));
        module.extend(capability(FRAGMENT_BARYCENTRIC));
        module.extend([(2 << 16) | 10, u32::from(b'x')]);
        assert!(declares_capability(&module, FRAGMENT_BARYCENTRIC));
        assert!(!declares_capability(&module, 4472));
        // The same number later in the module is an operand, not a capability.
        let mut other = header.to_vec();
        other.extend(capability(1));
        other.extend([(2 << 16) | 10, FRAGMENT_BARYCENTRIC]);
        assert!(!declares_capability(&other, FRAGMENT_BARYCENTRIC));
        assert!(!declares_capability(&header, FRAGMENT_BARYCENTRIC));
    }
}
