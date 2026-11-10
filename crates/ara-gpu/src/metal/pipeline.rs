//! Metal 4 pipeline states. Engine pipelines come only from cooked, hashed
//! Metal Shader Converter metallibs (`CookedStage::metal`); the backend never compiles
//! shader source. The raster state mirrors the Vulkan backend: 48-byte vertex ABI,
//! no culling, counter-clockwise front faces, reversed-Z `>=` depth, premultiplied
//! alpha on attachment zero.
use super::abi::{STAGE_IN_ATTRIBUTE_START, VERTEX_BUFFER_BIND_POINT};
use super::cache::Compiler;
use super::native::{nil, ns_error, raw_format};
use crate::cooked::{CookedPipeline, CookedStage, RenderState, Stage, binding_layout_compatible};
use crate::{GpuError, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::*;
use sha2::{Digest, Sha256};
use std::path::Path;

/// A created pipeline and the encoder state it implies.
pub(super) enum NativePipeline {
    Render {
        state: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
        depth: Option<Retained<ProtocolObject<dyn MTLDepthStencilState>>>,
        colors: Vec<MTLPixelFormat>,
        depth_format: Option<MTLPixelFormat>,
        vertex_stride: u32,
        raster: crate::raster::RasterState,
    },
    Compute {
        state: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
        threads: MTLSize,
    },
}

/// The subset of converter reflection (`--output-reflection-file`) the runtime uses,
/// in the converter 4.0 shape: stage details live under `state`.
#[derive(serde::Deserialize)]
pub(super) struct Reflection {
    #[serde(rename = "EntryPoint")]
    pub entry_point: String,
    #[serde(default)]
    state: StageReflection,
}

/// Per-stage reflection details (`state`): compute thread-group size and whether a
/// vertex stage reads converter draw parameters.
#[derive(Default, serde::Deserialize)]
struct StageReflection {
    #[serde(default)]
    tg_size: Option<[u32; 3]>,
    #[serde(default)]
    needs_draw_params: bool,
}

/// Create a pipeline from cooked Metal artifacts; DXIL/SPIR-V are never loaded here.
pub(super) fn create(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &Compiler,
    cooked: &CookedPipeline,
    dir: &Path,
) -> Result<NativePipeline> {
    let libraries = cooked
        .stages
        .iter()
        .map(|stage| load(device, cooked, stage, dir))
        .collect::<Result<Vec<_>>>()?;
    build_loaded(device, compiler, &libraries, &cooked.state)
}

/// Create only the native PSO after library loading and hash validation have completed.
pub(super) fn build_loaded(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &Compiler,
    libraries: &[LoadedStage],
    state: &RenderState,
) -> Result<NativePipeline> {
    let functions = libraries
        .iter()
        .map(|(stage, library, reflection)| (*stage, &**library, reflection.entry_point.as_str()))
        .collect::<Vec<_>>();
    let threads = libraries.iter().find_map(|(_, _, r)| r.state.tg_size);
    build(device, compiler, &functions, state, threads)
}

/// A loaded, hash-checked stage library with its converter reflection.
pub(super) type LoadedStage = (Stage, Retained<ProtocolObject<dyn MTLLibrary>>, Reflection);

/// Load one cooked stage's metallib (hash checked) and converter reflection.
pub(super) fn load(
    device: &ProtocolObject<dyn MTLDevice>,
    cooked: &CookedPipeline,
    stage: &CookedStage,
    dir: &Path,
) -> Result<LoadedStage> {
    if cooked.version != 1 || !binding_layout_compatible(&cooked.binding_layout_hash) {
        return Err(GpuError::Contract(
            "Cooked layout/schema mismatch; recook shaders".into(),
        ));
    }
    let metal = stage.metal.as_ref().ok_or_else(|| {
        GpuError::Contract(format!(
            "Pipeline stage {} has no Metal artifacts; cook it with Metal Shader \
             Converter on the canonical cook host (see docs/dev/macos.md)",
            stage.source_entry
        ))
    })?;
    let bytes = std::fs::read(dir.join(&metal.metallib)).map_err(anyhow::Error::from)?;
    if format!("{:x}", Sha256::digest(&bytes)) != metal.metallib_hash {
        return Err(GpuError::Contract(format!(
            "Shader content hash mismatch: {}",
            metal.metallib
        )));
    }
    let reflection: Reflection = serde_json::from_slice(
        &std::fs::read(dir.join(&metal.reflection)).map_err(anyhow::Error::from)?,
    )
    .map_err(anyhow::Error::from)?;
    if reflection.state.needs_draw_params {
        // No cooked stage reads converter draw parameters; binding IRRuntimeDrawParams
        // and the index type at 4/5 waits for the first one that does.
        return Err(GpuError::Contract(format!(
            "{}: converter draw parameters are not supported by the Metal backend yet",
            stage.source_entry
        )));
    }
    let library = device
        .newLibraryWithData_error(&dispatch2::DispatchData::from_bytes(&bytes))
        .map_err(|e| ns_error("metallib load", &e))?;
    Ok((stage.stage, library, reflection))
}

/// A vertex-only render pipeline that writes depth and no color: `ClearDepth`.
/// Color attachments match the rendering scope with writes masked off.
pub(super) fn depth_only(
    compiler: &Compiler,
    library: &ProtocolObject<dyn MTLLibrary>,
    entry: &str,
    colors: &[MTLPixelFormat],
    samples: crate::texture::SampleCount,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>> {
    let function = MTL4LibraryFunctionDescriptor::new();
    function.setName(Some(&NSString::from_str(entry)));
    function.setLibrary(Some(library));
    let descriptor = MTL4RenderPipelineDescriptor::new();
    descriptor.setVertexFunctionDescriptor(Some(&function));
    // SAFETY: one and four are valid native raster sample counts.
    unsafe { descriptor.setRasterSampleCount(samples.count() as usize) };
    descriptor.setInputPrimitiveTopology(MTLPrimitiveTopologyClass::Triangle);
    let attachments = descriptor.colorAttachments();
    for (index, pixel) in colors.iter().enumerate() {
        // SAFETY: rendering scopes have at most Metal's eight color attachments.
        let attachment = unsafe { attachments.objectAtIndexedSubscript(index) };
        attachment.setPixelFormat(*pixel);
        attachment.setWriteMask(MTLColorWriteMask::None);
    }
    compiler
        .render(&descriptor)
        .map_err(|e| ns_error("depth-only pipeline", &e))
}

/// Build a pipeline from loaded libraries; shared by cooked artifacts and tests.
pub(super) fn build(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &Compiler,
    functions: &[(Stage, &ProtocolObject<dyn MTLLibrary>, &str)],
    state: &RenderState,
    threads: Option<[u32; 3]>,
) -> Result<NativePipeline> {
    state.raster.validate()?;
    let function = |library: &ProtocolObject<dyn MTLLibrary>, name: &str| {
        let descriptor = MTL4LibraryFunctionDescriptor::new();
        descriptor.setName(Some(&NSString::from_str(name)));
        descriptor.setLibrary(Some(library));
        descriptor
    };
    match functions {
        [(Stage::Compute, library, name)] => {
            let [x, y, z] = threads.ok_or_else(|| {
                GpuError::Contract("Compute pipeline has no thread-group size".into())
            })?;
            let descriptor = MTL4ComputePipelineDescriptor::new();
            descriptor.setComputeFunctionDescriptor(Some(&function(library, name)));
            let state = compiler
                .compute(&descriptor)
                .map_err(|e| ns_error("compute pipeline", &e))?;
            Ok(NativePipeline::Compute {
                state,
                threads: MTLSize {
                    width: x as usize,
                    height: y as usize,
                    depth: z as usize,
                },
            })
        }
        [
            (Stage::Vertex, vertex, vertex_name),
            (Stage::Fragment, fragment, fragment_name),
        ]
        | [
            (Stage::Fragment, fragment, fragment_name),
            (Stage::Vertex, vertex, vertex_name),
        ] => render(
            device,
            compiler,
            &function(vertex, vertex_name),
            &function(fragment, fragment_name),
            state,
        ),
        _ => Err(GpuError::Contract(
            "Metal backend supports compute and vertex/fragment pipelines; mesh/task \
             stages need the mesh tier"
                .into(),
        )),
    }
}

fn render(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &Compiler,
    vertex: &MTL4LibraryFunctionDescriptor,
    fragment: &MTL4LibraryFunctionDescriptor,
    state: &RenderState,
) -> Result<NativePipeline> {
    let descriptor = MTL4RenderPipelineDescriptor::new();
    descriptor.setVertexFunctionDescriptor(Some(vertex));
    descriptor.setFragmentFunctionDescriptor(Some(fragment));
    // SAFETY: the portable sample enum permits only native counts one and four.
    unsafe { descriptor.setRasterSampleCount(state.raster.samples.count() as usize) };
    descriptor.setInputPrimitiveTopology(MTLPrimitiveTopologyClass::Triangle);
    let colors = state
        .color_formats
        .iter()
        .map(|raw| raw_format(*raw))
        .collect::<Result<Vec<_>>>()?;
    let attachments = descriptor.colorAttachments();
    for (index, pixel) in colors.iter().enumerate() {
        // SAFETY: Metal supports eight color attachments; cooked states are validated
        // by the cook, and indices beyond the array would raise before writing.
        let attachment = unsafe { attachments.objectAtIndexedSubscript(index) };
        attachment.setPixelFormat(*pixel);
        if let Some(blend) = state.raster.blends.get(index) {
            if let Some(blend) = blend {
                attachment.setBlendingState(MTL4BlendState::Enabled);
                attachment.setSourceRGBBlendFactor(blend_factor(blend.color.source));
                attachment.setDestinationRGBBlendFactor(blend_factor(blend.color.destination));
                attachment.setRgbBlendOperation(blend_op(blend.color.operation));
                attachment.setSourceAlphaBlendFactor(blend_factor(blend.alpha.source));
                attachment.setDestinationAlphaBlendFactor(blend_factor(blend.alpha.destination));
                attachment.setAlphaBlendOperation(blend_op(blend.alpha.operation));
            }
        } else if state.blend && index == 0 {
            attachment.setBlendingState(MTL4BlendState::Enabled);
            attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
            attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            attachment.setRgbBlendOperation(MTLBlendOperation::Add);
            attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
            attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
        }
    }
    if !state.raster.vertex_layouts.is_empty() {
        use crate::raster::{VertexFormat as F, VertexStep};
        let layout = MTLVertexDescriptor::vertexDescriptor();
        for (binding, stream) in state.raster.vertex_layouts.iter().enumerate() {
            // SAFETY: validation limits streams to the argument table's reserved vertex slots.
            let target = unsafe {
                layout
                    .layouts()
                    .objectAtIndexedSubscript(VERTEX_BUFFER_BIND_POINT + binding)
            };
            // SAFETY: stride is nonzero, four-byte aligned and within the portable maximum.
            unsafe {
                target.setStride(stream.stride as usize);
                target.setStepRate(1);
            }
            target.setStepFunction(if stream.step == VertexStep::Instance {
                MTLVertexStepFunction::PerInstance
            } else {
                MTLVertexStepFunction::PerVertex
            });
            for a in &stream.attributes {
                // SAFETY: portable locations 0..16 map to valid converter attributes 11..27.
                let attr = unsafe {
                    layout
                        .attributes()
                        .objectAtIndexedSubscript(STAGE_IN_ATTRIBUTE_START + a.location as usize)
                };
                attr.setFormat(match a.format {
                    F::Float32 => MTLVertexFormat::Float,
                    F::Float32x2 => MTLVertexFormat::Float2,
                    F::Float32x3 => MTLVertexFormat::Float3,
                    F::Float32x4 => MTLVertexFormat::Float4,
                    F::Uint32 => MTLVertexFormat::UInt,
                    F::Uint32x2 => MTLVertexFormat::UInt2,
                    F::Uint32x3 => MTLVertexFormat::UInt3,
                    F::Uint32x4 => MTLVertexFormat::UInt4,
                    F::Unorm8x4 => MTLVertexFormat::UChar4Normalized,
                });
                // SAFETY: attribute fits inside its checked stream stride.
                unsafe {
                    attr.setOffset(a.offset as usize);
                    attr.setBufferIndex(VERTEX_BUFFER_BIND_POINT + binding);
                }
            }
        }
        descriptor.setVertexDescriptor(Some(&layout));
    } else if state.vertex_stride != 0 {
        let layout = MTLVertexDescriptor::vertexDescriptor();
        for (index, (format, offset)) in [
            (MTLVertexFormat::Float3, 0),
            (MTLVertexFormat::Float3, 12),
            (MTLVertexFormat::Float2, 24),
            (MTLVertexFormat::Float4, 32),
        ]
        .into_iter()
        .enumerate()
        {
            // SAFETY: attribute indices 11..15 are within Metal's 31-attribute array.
            let attribute = unsafe {
                layout
                    .attributes()
                    .objectAtIndexedSubscript(STAGE_IN_ATTRIBUTE_START + index)
            };
            attribute.setFormat(format);
            // SAFETY: offsets are fixed, aligned positions inside the 48-byte record.
            unsafe {
                attribute.setOffset(offset);
                attribute.setBufferIndex(VERTEX_BUFFER_BIND_POINT);
            }
        }
        // SAFETY: buffer index 6 is within Metal's 31 vertex-buffer layouts.
        let stream = unsafe {
            layout
                .layouts()
                .objectAtIndexedSubscript(VERTEX_BUFFER_BIND_POINT)
        };
        // SAFETY: the stride is the cooked, nonzero vertex record size.
        unsafe { stream.setStride(state.vertex_stride as usize) };
        descriptor.setVertexDescriptor(Some(&layout));
    }
    let pipeline = compiler
        .render(&descriptor)
        .map_err(|e| ns_error("render pipeline", &e))?;
    let depth_format = state.depth_format.map(raw_format).transpose()?;
    let depth = match depth_format {
        Some(_) => {
            let stencil = MTLDepthStencilDescriptor::new();
            stencil.setDepthCompareFunction(if state.depth_test {
                super::native::compare(state.raster.depth_compare)
            } else {
                MTLCompareFunction::Always
            });
            stencil.setDepthWriteEnabled(state.depth_write);
            Some(
                device
                    .newDepthStencilStateWithDescriptor(&stencil)
                    .ok_or_else(|| nil("depth state"))?,
            )
        }
        None => None,
    };
    Ok(NativePipeline::Render {
        state: pipeline,
        depth,
        colors,
        depth_format,
        vertex_stride: state.vertex_stride,
        raster: state.raster.clone(),
    })
}

fn blend_factor(f: crate::raster::BlendFactor) -> MTLBlendFactor {
    use crate::raster::BlendFactor as F;
    match f {
        F::Zero => MTLBlendFactor::Zero,
        F::One => MTLBlendFactor::One,
        F::SrcColor => MTLBlendFactor::SourceColor,
        F::OneMinusSrcColor => MTLBlendFactor::OneMinusSourceColor,
        F::DstColor => MTLBlendFactor::DestinationColor,
        F::OneMinusDstColor => MTLBlendFactor::OneMinusDestinationColor,
        F::SrcAlpha => MTLBlendFactor::SourceAlpha,
        F::OneMinusSrcAlpha => MTLBlendFactor::OneMinusSourceAlpha,
        F::DstAlpha => MTLBlendFactor::DestinationAlpha,
        F::OneMinusDstAlpha => MTLBlendFactor::OneMinusDestinationAlpha,
    }
}
fn blend_op(o: crate::raster::BlendOp) -> MTLBlendOperation {
    use crate::raster::BlendOp as O;
    match o {
        O::Add => MTLBlendOperation::Add,
        O::Subtract => MTLBlendOperation::Subtract,
        O::ReverseSubtract => MTLBlendOperation::ReverseSubtract,
        O::Min => MTLBlendOperation::Min,
        O::Max => MTLBlendOperation::Max,
    }
}
