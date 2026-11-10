//! Validate portable commands before allocating or recording native work.
use super::*;
impl VulkanDevice {
    fn validate_attachment_formats(
        &self,
        bound: Option<Pipeline>,
        colors: &[vk::Format],
        depth: Option<vk::Format>,
        samples: crate::texture::SampleCount,
    ) -> Result<()> {
        let handle = bound.ok_or_else(|| GpuError::Contract("No bound pipeline".into()))?;
        let pipeline = self.pipelines.get(handle)?;
        if pipeline.colors != colors
            || pipeline.depth != depth
            || pipeline.raster.samples != samples
        {
            return Err(GpuError::Contract(
                "Pipeline and rendering attachment formats differ".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn validate_commands(&self, list: &CommandList) -> Result<()> {
        crate::cpu_scope!("GPU.validate_commands");
        let mut rendering = false;
        let mut pass = false;
        let mut pipeline = None;
        let mut bound = None;
        let mut color_formats = Vec::new();
        let mut depth_format = None;
        let mut samples = crate::texture::SampleCount::One;
        let mut extent = [0u32; 2];
        for c in &list.commands {
            match c {
                Command::Barrier(_) if rendering => {
                    return Err(GpuError::Contract(
                        "Barriers must be outside rendering scopes".into(),
                    ));
                }
                Command::Barrier(b) => match b.resource {
                    Resource::Buffer(h) => {
                        self.buffers.get(h)?;
                    }
                    Resource::AccelerationStructure(h) => {
                        self.accelerations.get(h)?;
                    }
                    Resource::Image(h) => {
                        self.images.get(h)?;
                    }
                },
                Command::RefitAccelerationStructure(h) => {
                    if rendering {
                        return Err(GpuError::Contract("AS refit inside rendering".into()));
                    }
                    self.validate_as_refit(*h)?;
                }
                Command::BuildAccelerationStructure(h) => {
                    if rendering {
                        return Err(GpuError::Contract("AS build inside rendering scope".into()));
                    }
                    self.validate_as_build(*h)?;
                }
                Command::Pipeline(h) => {
                    pipeline = Some(self.pipelines.get(*h)?.compute);
                    bound = Some(*h);
                    if list.queue == Queue::Compute && pipeline == Some(false) {
                        return Err(GpuError::Contract(
                            "Graphics pipeline on compute-only queue".into(),
                        ));
                    }
                }
                Command::BeginRendering {
                    colors,
                    depth,
                    size,
                } => {
                    if rendering || list.queue != Queue::Graphics || size.contains(&0) {
                        return Err(GpuError::Contract("Invalid rendering scope".into()));
                    }
                    let mut attachment_samples = None;
                    for a in colors.iter().chain(depth.iter()) {
                        let i = self.images.get(a.image)?;
                        if attachment_samples.is_some_and(|s| s != i.raw.shape.samples) {
                            return Err(GpuError::Contract(
                                "Attachment sample counts differ".into(),
                            ));
                        }
                        attachment_samples = Some(i.raw.shape.samples);
                        if size[0] > i.raw.width || size[1] > i.raw.height {
                            return Err(GpuError::Contract("Attachment extent too small".into()));
                        }
                    }
                    samples = attachment_samples.unwrap_or_default();
                    color_formats = colors
                        .iter()
                        .map(|a| self.images.get(a.image).map(|i| i.raw.format))
                        .collect::<Result<Vec<_>>>()?;
                    depth_format = depth
                        .as_ref()
                        .map(|a| self.images.get(a.image).map(|i| i.raw.format))
                        .transpose()?;
                    extent = *size;
                    rendering = true;
                }
                Command::ClearDepth {
                    origin,
                    size,
                    depth,
                } => {
                    let inside = (0..2).all(|i| {
                        origin[i]
                            .checked_add(size[i])
                            .is_some_and(|end| end <= extent[i])
                    });
                    if !rendering
                        || depth_format.is_none()
                        || size.contains(&0)
                        || !inside
                        || !depth.is_finite()
                    {
                        return Err(GpuError::Contract(
                            "Depth clear outside the rendering scope's depth attachment".into(),
                        ));
                    }
                }
                Command::EndRendering => {
                    if !rendering {
                        return Err(GpuError::Contract("No rendering scope".into()));
                    }
                    rendering = false;
                }
                Command::Draw(d) => {
                    self.validate_attachment_formats(bound, &color_formats, depth_format, samples)?;
                    if bound.is_some_and(|h| {
                        self.pipelines
                            .get(h)
                            .is_ok_and(|p| !p.raster.vertex_layouts.is_empty())
                    }) {
                        return Err(GpuError::Contract(
                            "Explicit layouts require DrawVertexBuffers".into(),
                        ));
                    }
                    let vertices = self.buffers.get(d.vertices)?;
                    if !vertices
                        .raw
                        .usage
                        .contains(vk::BufferUsageFlags::VERTEX_BUFFER)
                    {
                        return Err(GpuError::Contract("Vertex buffer usage missing".into()));
                    }
                    let i = self.buffers.get(d.indices)?;
                    if !i.raw.usage.contains(vk::BufferUsageFlags::INDEX_BUFFER) {
                        return Err(GpuError::Contract("Index buffer usage missing".into()));
                    }
                    if !rendering
                        || pipeline != Some(false)
                        || bound.is_some_and(|h| self.pipelines.get(h).is_ok_and(|p| p.mesh))
                        || (u64::from(d.first_index) + u64::from(d.index_count)) * 4 > i.raw.size
                    {
                        return Err(GpuError::Contract("Invalid indexed draw".into()));
                    }
                }
                Command::DrawVertexBuffers {
                    vertices,
                    indices,
                    first_index,
                    index_count,
                    instances,
                    first_instance,
                    ..
                } => {
                    self.validate_attachment_formats(bound, &color_formats, depth_format, samples)?;
                    let p = self.pipelines.get(
                        bound.ok_or_else(|| GpuError::Contract("Draw without pipeline".into()))?,
                    )?;
                    if !rendering
                        || p.compute
                        || p.mesh
                        || p.raster.vertex_layouts.is_empty()
                        || p.raster.vertex_layouts.len() != vertices.len()
                        || *instances == 0
                        || *index_count == 0
                    {
                        return Err(GpuError::Contract("Invalid vertex stream draw".into()));
                    }
                    for (binding, layout) in vertices.iter().zip(&p.raster.vertex_layouts) {
                        let b = self.buffers.get(binding.buffer)?.raw;
                        let records = if layout.step == crate::raster::VertexStep::Instance {
                            u64::from(*first_instance) + u64::from(*instances)
                        } else {
                            1
                        };
                        if !binding.offset.is_multiple_of(4)
                            || binding
                                .offset
                                .checked_add(records * u64::from(layout.stride))
                                .is_none_or(|end| end > b.size)
                            || !b.usage.contains(vk::BufferUsageFlags::VERTEX_BUFFER)
                        {
                            return Err(GpuError::Contract("Invalid vertex buffer range".into()));
                        }
                    }
                    let b = self.buffers.get(*indices)?.raw;
                    if (u64::from(*first_index) + u64::from(*index_count)) * 4 > b.size
                        || !b.usage.contains(vk::BufferUsageFlags::INDEX_BUFFER)
                    {
                        return Err(GpuError::Contract("Invalid index range".into()));
                    }
                }
                Command::DrawIndirect { arguments, offset }
                | Command::DrawMeshTasksIndirect { arguments, offset } => {
                    let mesh_draw = matches!(c, Command::DrawMeshTasksIndirect { .. });
                    self.validate_attachment_formats(bound, &color_formats, depth_format, samples)?;
                    let buffer = self.buffers.get(*arguments)?;
                    if !rendering
                        || pipeline != Some(false)
                        || bound.is_none_or(|h| {
                            self.pipelines.get(h).is_ok_and(|p| {
                                (p.vertex_stride != 0 || !p.raster.vertex_layouts.is_empty())
                                    || p.mesh != mesh_draw
                            })
                        })
                        || offset % 4 != 0
                        || offset
                            .checked_add(if mesh_draw { 12 } else { 16 })
                            .is_none_or(|end| end > buffer.raw.size)
                        || !buffer
                            .raw
                            .usage
                            .contains(vk::BufferUsageFlags::INDIRECT_BUFFER)
                    {
                        return Err(GpuError::Contract("Invalid indirect draw".into()));
                    }
                }
                Command::Fullscreen => {
                    self.validate_attachment_formats(bound, &color_formats, depth_format, samples)?;
                    if !rendering
                        || pipeline != Some(false)
                        || bound.is_some_and(|h| self.pipelines.get(h).is_ok_and(|p| p.mesh))
                    {
                        return Err(GpuError::Contract("Draw outside graphics scope".into()));
                    }
                }
                Command::DrawIndexedProcedural {
                    indices,
                    first_index,
                    count,
                    instances,
                } => {
                    self.validate_attachment_formats(bound, &color_formats, depth_format, samples)?;
                    let native = self
                        .pipelines
                        .get(bound.expect("attachment validation requires a bound pipeline"))?;
                    let i = self.buffers.get(*indices)?;
                    if !rendering
                        || pipeline != Some(false)
                        || native.vertex_stride != 0
                        || native.mesh
                        || *count == 0
                        || *instances == 0
                        || !i.raw.usage.contains(vk::BufferUsageFlags::INDEX_BUFFER)
                        || (u64::from(*first_index) + u64::from(*count)) * 4 > i.raw.size
                    {
                        return Err(GpuError::Contract("Invalid indexed procedural draw".into()));
                    }
                }
                Command::DrawProcedural {
                    vertices,
                    instances,
                } => {
                    self.validate_attachment_formats(bound, &color_formats, depth_format, samples)?;
                    let native = self
                        .pipelines
                        .get(bound.expect("attachment validation requires a bound pipeline"))?;
                    if !rendering
                        || pipeline != Some(false)
                        || native.vertex_stride != 0
                        || native.mesh
                        || *vertices == 0
                        || *instances == 0
                    {
                        return Err(GpuError::Contract(
                            "Invalid procedural graphics draw".into(),
                        ));
                    }
                }
                Command::Dispatch(_) => {
                    if rendering || pipeline != Some(true) {
                        return Err(GpuError::Contract("Dispatch outside compute scope".into()));
                    }
                }
                Command::DispatchIndirect { arguments, offset } => {
                    let buffer = self.buffers.get(*arguments)?.raw;
                    if rendering
                        || pipeline != Some(true)
                        || offset % 4 != 0
                        || offset.checked_add(12).is_none_or(|end| end > buffer.size)
                        || !buffer.usage.contains(vk::BufferUsageFlags::INDIRECT_BUFFER)
                    {
                        return Err(GpuError::Contract(
                            "Invalid indirect dispatch scope, usage or range".into(),
                        ));
                    }
                }
                Command::UploadImageRegion {
                    source,
                    destination,
                    buffer_offset,
                    region,
                } => {
                    let b = self.buffers.get(*source)?.raw;
                    let i = self.images.get(*destination)?.raw;
                    let _ = super::image_copy::buffer_region(i, *region, *buffer_offset, b.size)?;
                    if rendering
                        || !b.usage.contains(vk::BufferUsageFlags::TRANSFER_SRC)
                        || !i.usage.contains(vk::ImageUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract(
                            "Invalid image upload usage or scope".into(),
                        ));
                    }
                }
                Command::ReadImageRegion {
                    source,
                    destination,
                    buffer_offset,
                    region,
                } => {
                    let b = self.buffers.get(*destination)?.raw;
                    let i = self.images.get(*source)?.raw;
                    let _ = super::image_copy::buffer_region(i, *region, *buffer_offset, b.size)?;
                    if rendering
                        || !b.usage.contains(vk::BufferUsageFlags::TRANSFER_DST)
                        || !i.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
                    {
                        return Err(GpuError::Contract(
                            "Invalid image readback usage or scope".into(),
                        ));
                    }
                }
                Command::CopyImageRegion {
                    source,
                    destination,
                    from,
                    to,
                } => {
                    let a = self.images.get(*source)?.raw;
                    let b = self.images.get(*destination)?.raw;
                    let _ = super::image_copy::subresource(a, *from)?;
                    let _ = super::image_copy::subresource(b, *to)?;
                    if rendering
                        || source == destination
                        || a.format != b.format
                        || from.extent != to.extent
                        || from.layer_count != to.layer_count
                        || !a.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
                        || !b.usage.contains(vk::ImageUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract("Invalid image region copy".into()));
                    }
                }
                Command::UploadImage {
                    source,
                    destination,
                } => {
                    let b = self.buffers.get(*source)?.raw;
                    let i = self.images.get(*destination)?.raw;
                    if rendering
                        || b.size < copy_bytes(i)?
                        || !b.usage.contains(vk::BufferUsageFlags::TRANSFER_SRC)
                        || !i.usage.contains(vk::ImageUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract("Invalid buffer-to-image copy".into()));
                    }
                }
                Command::CopyBuffer {
                    source,
                    destination,
                    size,
                } => {
                    let source = self.buffers.get(*source)?.raw;
                    let dest = self.buffers.get(*destination)?.raw;
                    if rendering
                        || *size == 0
                        || size % 4 != 0
                        || *size > source.size
                        || *size > dest.size
                        || !source.usage.contains(vk::BufferUsageFlags::TRANSFER_SRC)
                        || !dest.usage.contains(vk::BufferUsageFlags::TRANSFER_DST)
                        || source.handle == dest.handle
                    {
                        return Err(GpuError::Contract("Invalid buffer copy".into()));
                    }
                }
                Command::CopyBufferRange {
                    source,
                    source_offset,
                    destination,
                    destination_offset,
                    size,
                } => {
                    let source = self.buffers.get(*source)?.raw;
                    let destination = self.buffers.get(*destination)?.raw;
                    if rendering
                        || *size == 0
                        || size % 4 != 0
                        || source_offset % 4 != 0
                        || destination_offset % 4 != 0
                        || source_offset
                            .checked_add(*size)
                            .is_none_or(|end| end > source.size)
                        || destination_offset
                            .checked_add(*size)
                            .is_none_or(|end| end > destination.size)
                        || !source.usage.contains(vk::BufferUsageFlags::TRANSFER_SRC)
                        || !destination
                            .usage
                            .contains(vk::BufferUsageFlags::TRANSFER_DST)
                        || source.handle == destination.handle
                    {
                        return Err(GpuError::Contract("Invalid buffer copy range".into()));
                    }
                }
                Command::FillBuffer {
                    destination,
                    offset,
                    size,
                    ..
                } => {
                    let destination = self.buffers.get(*destination)?.raw;
                    crate::transfers::validate_fill(*offset, *size, destination.size)?;
                    if rendering
                        || !destination
                            .usage
                            .contains(vk::BufferUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract(
                            "Invalid buffer fill scope or usage".into(),
                        ));
                    }
                }
                Command::UpdateBuffer {
                    destination,
                    offset,
                    bytes,
                } => {
                    let destination = self.buffers.get(*destination)?.raw;
                    if rendering
                        || bytes.is_empty()
                        || bytes.len() % 4 != 0
                        || offset % 4 != 0
                        || offset
                            .checked_add(bytes.len() as u64)
                            .is_none_or(|end| end > destination.size)
                        || !destination
                            .usage
                            .contains(vk::BufferUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract("Invalid buffer update".into()));
                    }
                }
                Command::ResolveImage {
                    source,
                    destination,
                } => {
                    let a = self.images.get(*source)?.raw;
                    let b = self.images.get(*destination)?.raw;
                    if rendering
                        || a.shape.samples != crate::texture::SampleCount::Four
                        || b.shape.samples != crate::texture::SampleCount::One
                        || a.width != b.width
                        || a.height != b.height
                        || a.format != b.format
                        || a.aspect != vk::ImageAspectFlags::COLOR
                        || a.shape.layers != 1
                        || b.shape.layers != 1
                        || !a.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
                        || !b.usage.contains(vk::ImageUsageFlags::TRANSFER_DST)
                        || !b.usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                    {
                        return Err(GpuError::Contract("Invalid color resolve".into()));
                    }
                }
                Command::CopyImage {
                    source,
                    destination,
                } => {
                    let source = self.images.get(*source)?.raw;
                    let destination = self.images.get(*destination)?.raw;
                    if rendering
                        || source.handle == destination.handle
                        || source.format != destination.format
                        || source.width != destination.width
                        || source.height != destination.height
                        || source.aspect != vk::ImageAspectFlags::COLOR
                        || !source.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
                        || !destination
                            .usage
                            .contains(vk::ImageUsageFlags::TRANSFER_DST)
                    {
                        return Err(GpuError::Contract("Invalid image copy".into()));
                    }
                }
                Command::ReadImage {
                    source,
                    destination,
                } => {
                    let i = self.images.get(*source)?.raw;
                    let b = self.buffers.get(*destination)?.raw;
                    if rendering
                        || b.size < copy_bytes(i)?
                        || !b.usage.contains(vk::BufferUsageFlags::TRANSFER_DST)
                        || !i.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
                    {
                        return Err(GpuError::Contract("Invalid image-to-buffer copy".into()));
                    }
                }
                Command::BeginPass(_) => {
                    if pass {
                        return Err(GpuError::Contract("Nested measured pass".into()));
                    }
                    pass = true;
                }
                Command::EndPass => {
                    if !pass {
                        return Err(GpuError::Contract("No measured pass".into()));
                    }
                    pass = false;
                }
                _ => {}
            }
        }
        if pass || rendering {
            return Err(GpuError::Contract("Unclosed command scope".into()));
        }
        Ok(())
    }
}
