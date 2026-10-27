//! Metal 4 pipeline states. Engine pipelines come only from cooked, hashed
//! Metal Shader Converter metallibs (`CookedStage::metal`); the backend never compiles
//! shader source. The raster state mirrors the Vulkan backend: 48-byte vertex ABI,
//! no culling, counter-clockwise front faces, reversed-Z `>=` depth, premultiplied
//! alpha on attachment zero.
use super::abi::{STAGE_IN_ATTRIBUTE_START, VERTEX_BUFFER_BIND_POINT};
use super::native::{nil, ns_error, raw_format};
use crate::cooked::{CookedPipeline, RenderState, Stage, binding_layout_hash};
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
    },
    Compute {
        state: Retained<ProtocolObject<dyn MTLComputePipelineState>>,
        threads: MTLSize,
    },
}

/// The subset of converter reflection (`--output-reflection-file`) the runtime uses,
/// with the field names Apple's reflection guide documents.
#[derive(serde::Deserialize)]
struct Reflection {
    #[serde(rename = "EntryPoint")]
    entry_point: String,
    #[serde(default)]
    tg_size: Option<[u32; 3]>,
    #[serde(default)]
    needs_draw_params: bool,
}

/// Create a pipeline from cooked Metal artifacts; DXIL/SPIR-V are never loaded here.
pub(super) fn create(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    cooked: &CookedPipeline,
    dir: &Path,
) -> Result<NativePipeline> {
    if cooked.version != 1 || cooked.binding_layout_hash != binding_layout_hash() {
        return Err(GpuError::Contract(
            "Cooked layout/schema mismatch; recook shaders".into(),
        ));
    }
    let mut libraries = Vec::new();
    for stage in &cooked.stages {
        let metal = stage.metal.as_ref().ok_or_else(|| {
            GpuError::Contract(format!(
                "Pipeline stage {} has no Metal artifacts; cook it with Metal Shader \
                 Converter (see docs/dev/macos.md)",
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
        if reflection.needs_draw_params {
            // The draw-parameter buffers (bind points 4/5) are defined only by the
            // converter's runtime header; this lane is enabled once it is verified.
            return Err(GpuError::Contract(format!(
                "{}: converter draw parameters are not supported by the Metal backend yet",
                stage.source_entry
            )));
        }
        let library = device
            .newLibraryWithData_error(&dispatch2::DispatchData::from_bytes(&bytes))
            .map_err(|e| ns_error("metallib load", &e))?;
        libraries.push((stage.stage, library, reflection));
    }
    let functions = libraries
        .iter()
        .map(|(stage, library, reflection)| (*stage, &**library, reflection.entry_point.as_str()))
        .collect::<Vec<_>>();
    let threads = libraries.iter().find_map(|(_, _, r)| r.tg_size);
    build(device, compiler, &functions, &cooked.state, threads)
}

/// Build a pipeline from loaded libraries; shared by cooked artifacts and tests.
pub(super) fn build(
    device: &ProtocolObject<dyn MTLDevice>,
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    functions: &[(Stage, &ProtocolObject<dyn MTLLibrary>, &str)],
    state: &RenderState,
    threads: Option<[u32; 3]>,
) -> Result<NativePipeline> {
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
                .newComputePipelineStateWithDescriptor_compilerTaskOptions_error(&descriptor, None)
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
    compiler: &ProtocolObject<dyn MTL4Compiler>,
    vertex: &MTL4LibraryFunctionDescriptor,
    fragment: &MTL4LibraryFunctionDescriptor,
    state: &RenderState,
) -> Result<NativePipeline> {
    let descriptor = MTL4RenderPipelineDescriptor::new();
    descriptor.setVertexFunctionDescriptor(Some(vertex));
    descriptor.setFragmentFunctionDescriptor(Some(fragment));
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
        if state.blend && index == 0 {
            attachment.setBlendingState(MTL4BlendState::Enabled);
            attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
            attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            attachment.setRgbBlendOperation(MTLBlendOperation::Add);
            attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
            attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
        }
    }
    if state.vertex_stride != 0 {
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
        .newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&descriptor, None)
        .map_err(|e| ns_error("render pipeline", &e))?;
    let depth_format = state.depth_format.map(raw_format).transpose()?;
    let depth = match depth_format {
        Some(_) => {
            let stencil = MTLDepthStencilDescriptor::new();
            stencil.setDepthCompareFunction(if state.depth_test {
                MTLCompareFunction::GreaterEqual
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
    })
}
