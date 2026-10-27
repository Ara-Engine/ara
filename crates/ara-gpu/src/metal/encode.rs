//! Lowering of an owned [`CommandList`] onto one Metal 4 command buffer.
//!
//! Rendering scopes become render encoders; dispatches, copies and updates share one
//! compute encoder between scopes. Metal 4 has no automatic hazard tracking, so graph
//! barriers lower to stage barriers: an encoder barrier inside a compute encoder, and
//! a queue barrier on the next encoder otherwise. The first encoder of every
//! submission also waits on earlier queue work, which keeps Vulkan's cross-submission
//! barrier semantics conservative. Each draw and dispatch binds a fresh top-level
//! argument buffer (root constants plus table addresses) through the argument table.
use super::MetalDevice;
use super::abi::{ARGUMENT_BUFFER_BIND_POINT, VERTEX_BUFFER_BIND_POINT};
use super::native::{level_bytes, nil};
use super::pipeline::NativePipeline;
use super::scratch::{Chunk, Scratch};
use crate::commands::{Attachment, Command, CommandList};
use crate::{GpuError, Pipeline, Queue, Result};
use ash::vk;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::*;

/// Stages a Metal 4 compute encoder runs: dispatches, copies, AS builds.
const COMPUTE_STAGES: MTLStages = MTLStages::Dispatch
    .union(MTLStages::Blit)
    .union(MTLStages::AccelerationStructure);
/// Stages a Metal 4 render encoder runs.
const RENDER_STAGES: MTLStages = MTLStages::Vertex
    .union(MTLStages::Fragment)
    .union(MTLStages::Object)
    .union(MTLStages::Mesh);

/// Draw families with different vertex-input rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DrawKind {
    /// Indexed draw over the 48-byte vertex stream.
    Indexed,
    /// Non-indexed draws that fetch vertex data from storage (no vertex input).
    Procedural,
    /// Fullscreen triangle; any graphics pipeline.
    Fullscreen,
}

enum Active {
    None,
    Compute(Retained<ProtocolObject<dyn MTL4ComputeCommandEncoder>>),
    Render(Retained<ProtocolObject<dyn MTL4RenderCommandEncoder>>),
}

/// Recording state for one submission.
struct Recording<'a> {
    cmd: &'a ProtocolObject<dyn MTL4CommandBuffer>,
    scratch: &'a mut Scratch,
    created: &'a mut Vec<Chunk>,
    active: Active,
    /// A graph barrier (or the submission start) awaits the next encoder.
    barrier: bool,
    pipeline: Option<Pipeline>,
    bound: Option<Pipeline>,
    root: [u32; 8],
    /// Color and depth formats of the open rendering scope.
    formats: (Vec<MTLPixelFormat>, Option<MTLPixelFormat>),
}

impl MetalDevice {
    /// Encode `list` into `cmd`. Errors leave `cmd` uncommitted; partially recorded
    /// commands are discarded with it.
    pub(super) fn encode(
        &mut self,
        cmd: &ProtocolObject<dyn MTL4CommandBuffer>,
        list: &CommandList,
        scratch: &mut Scratch,
        created: &mut Vec<Chunk>,
    ) -> Result<()> {
        let mut rec = Recording {
            cmd,
            scratch,
            created,
            active: Active::None,
            barrier: true,
            pipeline: None,
            bound: None,
            root: [0; 8],
            formats: (Vec::new(), None),
        };
        let result = list
            .commands
            .iter()
            .try_for_each(|command| self.encode_one(&mut rec, list.queue, command));
        rec.end();
        result
    }

    fn encode_one(&mut self, rec: &mut Recording, queue: Queue, command: &Command) -> Result<()> {
        match command {
            Command::Barrier(_) => {
                if matches!(rec.active, Active::Render(_)) {
                    return Err(GpuError::Contract(
                        "Barriers must be outside rendering scopes".into(),
                    ));
                }
                // Order later work in this encoder, and in any encoder opened next.
                if let Active::Compute(encoder) = &rec.active {
                    encoder.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                        COMPUTE_STAGES,
                        COMPUTE_STAGES,
                        MTL4VisibilityOptions::Device,
                    );
                }
                rec.barrier = true;
            }
            Command::Pipeline(pipeline) => {
                self.pipelines.get(*pipeline)?;
                rec.pipeline = Some(*pipeline);
            }
            Command::Constants(constants) => rec.root = constants.words,
            Command::Dispatch([x, y, z]) => {
                let threads = self.bind_compute(rec)?;
                let groups = MTLSize {
                    width: *x as usize,
                    height: *y as usize,
                    depth: *z as usize,
                };
                rec.compute(self)?
                    .dispatchThreadgroups_threadsPerThreadgroup(groups, threads);
            }
            Command::DispatchIndirect { arguments, offset } => {
                let address = self.indirect_address(*arguments, *offset, 12)?;
                let threads = self.bind_compute(rec)?;
                // SAFETY: the 12-byte record lies inside the resident argument buffer.
                unsafe {
                    rec.compute(self)?
                        .dispatchThreadgroupsWithIndirectBuffer_threadsPerThreadgroup(
                            address, threads,
                        );
                }
            }
            Command::BeginRendering {
                colors,
                depth,
                size,
            } => {
                if queue != Queue::Graphics
                    || size.contains(&0)
                    || matches!(rec.active, Active::Render(_))
                {
                    return Err(GpuError::Contract("Invalid rendering scope".into()));
                }
                rec.end();
                let encoder = self.begin_rendering(rec, colors, depth.as_ref(), *size)?;
                rec.active = Active::Render(encoder);
                rec.bound = None;
            }
            Command::Viewport { origin, size } => {
                let encoder = rec.render()?;
                encoder.setViewport(MTLViewport {
                    originX: f64::from(origin[0]),
                    originY: f64::from(origin[1]),
                    width: f64::from(size[0]),
                    height: f64::from(size[1]),
                    znear: 0.,
                    zfar: 1.,
                });
                encoder.setScissorRect(MTLScissorRect {
                    x: origin[0] as usize,
                    y: origin[1] as usize,
                    width: size[0] as usize,
                    height: size[1] as usize,
                });
            }
            Command::Draw(draw) => {
                let indices = &self.buffers.get(draw.indices)?.raw;
                let vertices = &self.buffers.get(draw.vertices)?.raw;
                let first = u64::from(draw.first_index) * 4;
                let end = first + u64::from(draw.index_count) * 4;
                if draw.index_count == 0
                    || end > indices.size
                    || !indices.usage.contains(vk::BufferUsageFlags::INDEX_BUFFER)
                    || !vertices.usage.contains(vk::BufferUsageFlags::VERTEX_BUFFER)
                {
                    return Err(GpuError::Contract("Invalid indexed draw".into()));
                }
                let index_address = indices.buffer.gpuAddress() + first;
                let vertices = vertices.buffer.gpuAddress();
                self.bind_render(rec, DrawKind::Indexed)?;
                // SAFETY: the vertex stream address belongs to a live, resident buffer.
                unsafe {
                    self.table
                        .setAddress_atIndex(vertices, VERTEX_BUFFER_BIND_POINT)
                };
                // SAFETY: the checked index range lies inside the resident index buffer.
                unsafe {
                    rec.render()?
                        .drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferLength(
                            MTLPrimitiveType::Triangle,
                            draw.index_count as usize,
                            MTLIndexType::UInt32,
                            index_address,
                            (end - first) as usize,
                        );
                }
            }
            Command::DrawIndirect { arguments, offset } => {
                let address = self.indirect_address(*arguments, *offset, 16)?;
                self.bind_render(rec, DrawKind::Procedural)?;
                rec.render()?
                    .drawPrimitives_indirectBuffer(MTLPrimitiveType::Triangle, address);
            }
            Command::Fullscreen => {
                self.bind_render(rec, DrawKind::Fullscreen)?;
                // SAFETY: a three-vertex draw with no vertex input.
                unsafe {
                    rec.render()?.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        3,
                    );
                }
            }
            Command::DrawProcedural {
                vertices,
                instances,
            } => {
                if *vertices == 0 || *instances == 0 {
                    return Err(GpuError::Contract("Empty procedural draw".into()));
                }
                self.bind_render(rec, DrawKind::Procedural)?;
                // SAFETY: a procedural draw with no vertex input.
                unsafe {
                    rec.render()?
                        .drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                            MTLPrimitiveType::Triangle,
                            0,
                            *vertices as usize,
                            *instances as usize,
                            0,
                        );
                }
            }
            Command::EndRendering => {
                rec.render()?;
                rec.end();
            }
            Command::CopyBuffer {
                source,
                destination,
                size,
            } => self.copy_buffer(rec, *source, 0, *destination, 0, *size)?,
            Command::CopyBufferRange {
                source,
                source_offset,
                destination,
                destination_offset,
                size,
            } => self.copy_buffer(
                rec,
                *source,
                *source_offset,
                *destination,
                *destination_offset,
                *size,
            )?,
            Command::UpdateBuffer {
                destination,
                offset,
                bytes,
            } => {
                if bytes.is_empty()
                    || bytes.len() > 65_536
                    || !bytes.len().is_multiple_of(4)
                    || !offset.is_multiple_of(4)
                {
                    return Err(GpuError::Contract("Invalid ordered buffer update".into()));
                }
                let (chunk, at) =
                    rec.scratch
                        .write(&mut self.scratch, &self.device, &mut *rec.created, bytes)?;
                let target = self.buffers.get(*destination)?;
                if offset + bytes.len() as u64 > target.raw.size {
                    return Err(GpuError::Contract("Update exceeds buffer".into()));
                }
                let encoder = rec.compute(self)?;
                encoder.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                    COMPUTE_STAGES,
                    MTLStages::Blit,
                    MTL4VisibilityOptions::Device,
                );
                // SAFETY: both ranges were bounds-checked and both buffers are resident.
                unsafe {
                    encoder.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(
                        &chunk,
                        at as usize,
                        &target.raw.buffer,
                        *offset as usize,
                        bytes.len(),
                    );
                }
                encoder.barrierAfterEncoderStages_beforeEncoderStages_visibilityOptions(
                    MTLStages::Blit,
                    COMPUTE_STAGES,
                    MTL4VisibilityOptions::Device,
                );
            }
            Command::UploadImage {
                source,
                destination,
            } => self.copy_image_buffer(rec, *source, *destination, true)?,
            Command::ReadImage {
                source,
                destination,
            } => self.copy_image_buffer(rec, *destination, *source, false)?,
            Command::CopyImage {
                source,
                destination,
            } => {
                let (from, to) = (self.images.get(*source)?, self.images.get(*destination)?);
                if source == destination
                    || from.raw.size != to.raw.size
                    || from.raw.info.pixel != to.raw.info.pixel
                {
                    return Err(GpuError::Contract("Incompatible image copy".into()));
                }
                let (from, to) = (from.raw.texture.clone(), to.raw.texture.clone());
                // SAFETY: equally sized and formatted mip zero of two resident textures.
                unsafe {
                    rec.compute(self)?
                        .copyFromTexture_sourceSlice_sourceLevel_toTexture_destinationSlice_destinationLevel_sliceCount_levelCount(
                            &from, 0, 0, &to, 0, 0, 1, 1,
                        );
                }
            }
            Command::BeginPass(label) => rec.cmd.pushDebugGroup(&NSString::from_str(label)),
            Command::EndPass => rec.cmd.popDebugGroup(),
            Command::ClearDepth { .. } => {
                return Err(GpuError::Contract(
                    "ClearDepth needs a cooked clear pipeline on Metal (not yet available)".into(),
                ));
            }
            Command::BuildAccelerationStructure(_) | Command::DrawMeshTasksIndirect { .. } => {
                return Err(GpuError::Contract(
                    "Ray tracing and mesh tiers are not available on the Metal backend yet".into(),
                ));
            }
        }
        Ok(())
    }

    fn begin_rendering(
        &self,
        rec: &mut Recording,
        colors: &[Attachment],
        depth: Option<&Attachment>,
        size: [u32; 2],
    ) -> Result<Retained<ProtocolObject<dyn MTL4RenderCommandEncoder>>> {
        let pass = MTL4RenderPassDescriptor::new();
        pass.setRenderTargetWidth(size[0] as usize);
        pass.setRenderTargetHeight(size[1] as usize);
        for (index, attachment) in colors.iter().enumerate() {
            let image = self.images.get(attachment.image)?;
            if image.raw.info.depth || image.raw.size != size {
                return Err(GpuError::Contract("Color attachment mismatch".into()));
            }
            // SAFETY: Metal supports eight color attachments; more fails validation.
            let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(index) };
            color.setTexture(Some(&image.raw.texture));
            color.setStoreAction(MTLStoreAction::Store);
            match attachment.clear {
                Some([r, g, b, a]) => {
                    color.setLoadAction(MTLLoadAction::Clear);
                    color.setClearColor(MTLClearColor {
                        red: f64::from(r),
                        green: f64::from(g),
                        blue: f64::from(b),
                        alpha: f64::from(a),
                    });
                }
                None => color.setLoadAction(MTLLoadAction::Load),
            }
        }
        if let Some(attachment) = depth {
            let image = self.images.get(attachment.image)?;
            if !image.raw.info.depth || image.raw.size != size {
                return Err(GpuError::Contract("Depth attachment mismatch".into()));
            }
            let target = pass.depthAttachment();
            target.setTexture(Some(&image.raw.texture));
            target.setStoreAction(MTLStoreAction::Store);
            match attachment.clear {
                Some([value, ..]) => {
                    target.setLoadAction(MTLLoadAction::Clear);
                    target.setClearDepth(f64::from(value));
                }
                None => target.setLoadAction(MTLLoadAction::Load),
            }
        }
        let encoder = rec
            .cmd
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or_else(|| nil("render encoder"))?;
        let pixel = |a: &Attachment| self.images.get(a.image).map(|i| i.raw.info.pixel);
        rec.formats = (
            colors.iter().map(pixel).collect::<Result<_>>()?,
            depth.map(pixel).transpose()?,
        );
        if std::mem::take(&mut rec.barrier) {
            encoder.barrierAfterQueueStages_beforeStages_visibilityOptions(
                MTLStages::All,
                RENDER_STAGES,
                MTL4VisibilityOptions::Device,
            );
        }
        encoder.setArgumentTable_atStages(
            &self.table,
            MTLRenderStages::Vertex | MTLRenderStages::Fragment,
        );
        encoder.setFrontFacingWinding(MTLWinding::CounterClockwise);
        encoder.setCullMode(MTLCullMode::None);
        encoder.setViewport(MTLViewport {
            originX: 0.,
            originY: 0.,
            width: f64::from(size[0]),
            height: f64::from(size[1]),
            znear: 0.,
            zfar: 1.,
        });
        Ok(encoder)
    }

    /// Bind the current graphics pipeline and a fresh top-level argument buffer.
    /// Validates the pipeline against the rendering scope and draw kind, as the
    /// Vulkan backend does before native recording.
    fn bind_render(&mut self, rec: &mut Recording, kind: DrawKind) -> Result<()> {
        let handle = rec
            .pipeline
            .ok_or_else(|| GpuError::Contract("Draw without a pipeline".into()))?;
        let NativePipeline::Render {
            state,
            depth,
            colors,
            depth_format,
            vertex_stride,
        } = self.pipelines.get(handle)?
        else {
            return Err(GpuError::Contract("Draw with a compute pipeline".into()));
        };
        let encoder = rec.render()?;
        if *colors != rec.formats.0 || *depth_format != rec.formats.1 {
            return Err(GpuError::Contract(
                "Pipeline and rendering attachment formats differ".into(),
            ));
        }
        if kind != DrawKind::Fullscreen && (*vertex_stride != 0) != (kind == DrawKind::Indexed) {
            return Err(GpuError::Contract(
                "Draw kind does not match the pipeline vertex input".into(),
            ));
        }
        if rec.bound != Some(handle) {
            encoder.setRenderPipelineState(state);
            encoder.setDepthStencilState(depth.as_deref());
            rec.bound = Some(handle);
        }
        self.bind_top_level(rec)
    }

    /// Bind the current compute pipeline and a fresh top-level argument buffer.
    fn bind_compute(&mut self, rec: &mut Recording) -> Result<MTLSize> {
        let handle = rec
            .pipeline
            .ok_or_else(|| GpuError::Contract("Dispatch without a pipeline".into()))?;
        let NativePipeline::Compute { state, threads } = self.pipelines.get(handle)? else {
            return Err(GpuError::Contract(
                "Dispatch with a graphics pipeline".into(),
            ));
        };
        let (state, threads) = (state.clone(), *threads);
        if rec.bound != Some(handle) {
            rec.compute(self)?.setComputePipelineState(&state);
            rec.bound = Some(handle);
        }
        self.bind_top_level(rec)?;
        Ok(threads)
    }

    fn bind_top_level(&mut self, rec: &mut Recording) -> Result<()> {
        let top = self.tables.top_level(rec.root);
        let (chunk, offset) = rec.scratch.write(
            &mut self.scratch,
            &self.device,
            &mut *rec.created,
            bytemuck::bytes_of(&top),
        )?;
        // SAFETY: the scratch chunk is resident and retained until this submission
        // completes; the encoder snapshots argument-table bindings at each draw/dispatch.
        unsafe {
            self.table
                .setAddress_atIndex(chunk.gpuAddress() + offset, ARGUMENT_BUFFER_BIND_POINT);
        }
        Ok(())
    }

    /// GPU address of an indirect argument record in an `INDIRECT_BUFFER` buffer.
    fn indirect_address(&self, buffer: crate::Buffer, offset: u64, len: u64) -> Result<u64> {
        let raw = &self.buffers.get(buffer)?.raw;
        if !offset.is_multiple_of(4)
            || offset.checked_add(len).is_none_or(|end| end > raw.size)
            || !raw.usage.contains(vk::BufferUsageFlags::INDIRECT_BUFFER)
        {
            return Err(GpuError::Contract("Indirect record exceeds buffer".into()));
        }
        Ok(raw.buffer.gpuAddress() + offset)
    }

    fn copy_buffer(
        &self,
        rec: &mut Recording,
        source: crate::Buffer,
        source_offset: u64,
        destination: crate::Buffer,
        destination_offset: u64,
        size: u64,
    ) -> Result<()> {
        let (from, to) = (self.buffers.get(source)?, self.buffers.get(destination)?);
        let fits =
            |offset: u64, total: u64| offset.checked_add(size).is_some_and(|end| end <= total);
        if source == destination
            || size == 0
            || !size.is_multiple_of(4)
            || !source_offset.is_multiple_of(4)
            || !destination_offset.is_multiple_of(4)
            || !fits(source_offset, from.raw.size)
            || !fits(destination_offset, to.raw.size)
        {
            return Err(GpuError::Contract("Invalid buffer copy".into()));
        }
        let (from, to) = (from.raw.buffer.clone(), to.raw.buffer.clone());
        // SAFETY: both ranges were bounds-checked; the buffers are distinct and resident.
        unsafe {
            rec.compute(self)?
                .copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(
                    &from,
                    source_offset as usize,
                    &to,
                    destination_offset as usize,
                    size as usize,
                );
        }
        Ok(())
    }

    /// Tightly packed buffer <-> image copies of every level, finest first.
    fn copy_image_buffer(
        &self,
        rec: &mut Recording,
        buffer: crate::Buffer,
        image: crate::Image,
        upload: bool,
    ) -> Result<()> {
        let (raw_buffer, raw_image) =
            (&self.buffers.get(buffer)?.raw, &self.images.get(image)?.raw);
        let total: u64 = (0..raw_image.levels)
            .map(|level| level_bytes(&raw_image.info, raw_image.size, level).1)
            .sum();
        if total > raw_buffer.size || raw_image.info.depth {
            return Err(GpuError::Contract("Image copy exceeds buffer".into()));
        }
        let (buffer, texture) = (raw_buffer.buffer.clone(), raw_image.texture.clone());
        let (info, size, levels) = (&raw_image.info, raw_image.size, raw_image.levels);
        let mut offset = 0;
        let encoder = rec.compute(self)?;
        for level in 0..levels {
            let (row, bytes) = level_bytes(info, size, level);
            let extent = MTLSize {
                width: (size[0] >> level).max(1) as usize,
                height: (size[1] >> level).max(1) as usize,
                depth: 1,
            };
            let origin = MTLOrigin { x: 0, y: 0, z: 0 };
            // SAFETY: the packed level lies inside the buffer (checked total) and the
            // region is the whole level of a resident texture.
            unsafe {
                if upload {
                    encoder.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                        &buffer, offset as usize, row as usize, bytes as usize, extent,
                        &texture, 0, level as usize, origin,
                    );
                } else {
                    encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                        &texture, 0, level as usize, origin, extent,
                        &buffer, offset as usize, row as usize, bytes as usize,
                    );
                }
            }
            offset += bytes;
        }
        Ok(())
    }
}

impl Recording<'_> {
    fn render(&self) -> Result<&ProtocolObject<dyn MTL4RenderCommandEncoder>> {
        match &self.active {
            Active::Render(encoder) => Ok(encoder),
            _ => Err(GpuError::Contract("Draw outside a rendering scope".into())),
        }
    }
    /// The open compute encoder, created (after any pending queue barrier) on demand.
    fn compute(
        &mut self,
        device: &MetalDevice,
    ) -> Result<&ProtocolObject<dyn MTL4ComputeCommandEncoder>> {
        if matches!(self.active, Active::Render(_)) {
            return Err(GpuError::Contract(
                "Compute and copy commands must be outside rendering scopes".into(),
            ));
        }
        if matches!(self.active, Active::None) {
            let encoder = self
                .cmd
                .computeCommandEncoder()
                .ok_or_else(|| nil("compute encoder"))?;
            if std::mem::take(&mut self.barrier) {
                encoder.barrierAfterQueueStages_beforeStages_visibilityOptions(
                    MTLStages::All,
                    COMPUTE_STAGES,
                    MTL4VisibilityOptions::Device,
                );
            }
            encoder.setArgumentTable(Some(&device.table));
            self.active = Active::Compute(encoder);
            self.bound = None;
        }
        match &self.active {
            Active::Compute(encoder) => Ok(encoder),
            _ => unreachable!("a compute encoder was just ensured"),
        }
    }
    fn end(&mut self) {
        match std::mem::replace(&mut self.active, Active::None) {
            Active::Compute(encoder) => encoder.endEncoding(),
            Active::Render(encoder) => encoder.endEncoding(),
            Active::None => {}
        }
    }
}
