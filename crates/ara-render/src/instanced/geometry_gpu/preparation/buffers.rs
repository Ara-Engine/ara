use super::*;
use ara_gpu::vulkan::{BufferPreparation, PreparationId};

pub(in crate::instanced::geometry_gpu) const STATIC: usize = 0;
pub(in crate::instanced::geometry_gpu) const STAGE: usize = 1;
pub(in crate::instanced::geometry_gpu) const SKIN0: usize = 2;
pub(in crate::instanced::geometry_gpu) const TABLE: usize = 5;

#[derive(Default)]
pub(in crate::instanced::geometry_gpu) struct Buffers {
    request: Option<ResourcePreparation>,
    roles: Vec<usize>,
    ticket: Option<PreparationId>,
    handles: [Option<Buffer>; 6],
    pub(super) ready: bool,
}

impl Buffers {
    pub(super) fn add(
        &mut self,
        role: usize,
        size: u64,
        usage: vk::BufferUsageFlags,
        host: bool,
        data: Vec<u8>,
    ) {
        self.roles.push(role);
        self.request
            .get_or_insert_with(ResourcePreparation::default)
            .buffers
            .push(BufferPreparation {
                size,
                usage,
                host,
                data: data.into(),
            });
    }
    pub(super) fn poll(&mut self, gpu: &mut VulkanDevice) -> Result<bool> {
        if self.ready || self.roles.is_empty() {
            self.ready = true;
            return Ok(true);
        }
        if self.ticket.is_none() {
            ensure!(
                self.request.is_some(),
                "native geometry preparation failed; discard this candidate"
            );
            self.ticket = gpu
                .prepare_resources(|| self.request.take().expect("unsubmitted geometry request"))?;
            return Ok(false);
        }
        let ticket = self.ticket.take().expect("geometry ticket checked");
        let Some(resources) = gpu.poll_prepared_resources(ticket)? else {
            self.ticket = Some(ticket);
            return Ok(false);
        };
        if !resources.images.is_empty() || resources.buffers.len() != self.roles.len() {
            for image in resources.images {
                gpu.retire(Resource::Image(image))?;
            }
            for buffer in resources.buffers {
                gpu.retire(Resource::Buffer(buffer))?;
            }
            anyhow::bail!("native prepared geometry resource count mismatch");
        }
        for (&role, buffer) in self.roles.iter().zip(resources.buffers) {
            self.handles[role] = Some(buffer);
        }
        self.ready = true;
        Ok(true)
    }
    pub(super) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        if let Some(ticket) = self.ticket {
            gpu.cancel_prepared_resources(ticket)?;
        }
        for buffer in self.handles.into_iter().flatten() {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        Ok(())
    }
}

pub(in crate::instanced::geometry_gpu) fn take_or_allocate(
    gpu: &mut VulkanDevice,
    role: usize,
    bytes: u64,
    usage: vk::BufferUsageFlags,
    host: bool,
    allocated: &mut Vec<Resource>,
    prepared: &mut Option<&mut Buffers>,
) -> Result<Buffer> {
    let buffer = match prepared {
        Some(prepared) => prepared.handles[role]
            .take()
            .ok_or_else(|| anyhow::anyhow!("native prepared geometry buffer missing"))?,
        None => gpu.create_buffer(bytes, usage, host)?,
    };
    allocated.push(Resource::Buffer(buffer));
    Ok(buffer)
}
