use super::*;
use ara_gpu::{Barrier, vk};
/// Diagnostic counters, read only after completed rendering.
#[derive(serde::Serialize)]
pub struct Stats {
    /// Instanced original triangle count; this is not rasterized work.
    pub source_triangles: u64,
    /// Clusters emitted across both occlusion phases.
    pub clusters_drawn: u32,
    /// Software-classified triangles.
    pub software_triangles: u32,
    /// Hardware-classified triangles.
    pub hardware_triangles: u32,
    /// Dropped work due to visible queue exhaustion; a nonzero value fails capture.
    pub overflow: u32,
    /// Instances rejected by the current camera frustum.
    pub instances_frustum_culled: u32,
    /// Instances deferred by the previous frame's HZB.
    pub instances_occluded_previous: u32,
    /// Deferred instances still hidden by the current frame's HZB.
    pub instances_occluded_current: u32,
    /// Selected clusters rejected by the current frustum.
    pub clusters_culled: u32,
    /// Full-res cluster pages currently uploaded.
    pub pages_resident: u32,
    /// Allocated cluster page bytes.
    pub page_pool_bytes: u64,
    /// Lowest complete resident asset level; nonzero means the page budget can
    /// force a coarser cut than the requested geometric error.
    pub resident_floor: u32,
}
impl GeometryView {
    /// Requested resource bytes, excluding driver/alignment overhead, pipeline
    /// caches and resources owned by other rendering features.
    pub fn memory_bytes(&self) -> u64 {
        let pixels = u64::from(self.size[0]) * u64::from(self.size[1]);
        let c = u64::from(self.cluster_count);
        let i = u64::from(self.instance_count);
        size_of::<Frame>() as u64
            + i * 68
            + c * 52
            + u64::from(self.group_count.max(1)) * 32
            + u64::from(self.residency.capacity) * 2048
            + u64::from(self.capacity) * 8
            + 128
            + pixels * 76
            + u64::from(self.size[0].div_ceil(16)) * u64::from(self.size[1].div_ceil(16)) * 4
            + (c + 8) * 8
            + 16
            + 16 * 516 * 4
    }
    /// Completed diagnostic SDR pixels, useful for deterministic golden checks.
    pub fn preview_bytes(&self, gpu: &mut VulkanDevice) -> Result<Vec<u8>> {
        self.read_image(gpu, self.images[3], 4)
    }
    /// Completed world positions and geometric displacement bounds, one per pixel.
    pub fn surface(&self, gpu: &mut VulkanDevice) -> Result<Vec<[f32; 4]>> {
        Ok(self
            .read_image(gpu, self.images[2], 16)?
            .as_chunks::<16>()
            .0
            .iter()
            .map(|b| bytemuck::pod_read_unaligned(b))
            .collect())
    }
    /// Diagnostic readback of the final preview. Production callers sample outputs.
    pub fn capture(&self, gpu: &mut VulkanDevice, path: &Path) -> Result<()> {
        let bytes = self.read_image(gpu, self.images[3], 4)?;
        image::save_buffer(
            path,
            &bytes,
            self.size[0],
            self.size[1],
            image::ColorType::Rgba8,
        )?;
        Ok(())
    }
    /// Diagnostic U64 visibility identities; zero marks background.
    pub fn identities(&self, gpu: &mut VulkanDevice) -> Result<Vec<[u32; 2]>> {
        Ok(self
            .read_buffer(
                gpu,
                self.buffers[11],
                Access::ShaderRead,
                self.size[0] as usize * self.size[1] as usize * 8,
            )?
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bytes| bytemuck::pod_read_unaligned(bytes))
            .collect())
    }
    /// Reads counters without making them part of runtime submission.
    pub fn stats(&self, gpu: &mut VulkanDevice) -> Result<Stats> {
        let a = self.read_buffer(gpu, self.buffers[6], Access::ShaderRead, 128)?;
        let args: Vec<u32> = a
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| bytemuck::pod_read_unaligned(bytes))
            .collect();
        let f = self.read_buffer(
            gpu,
            self.buffers[9],
            Access::Storage,
            (self.cluster_count as usize + 8) * 4,
        )?;
        let feedback: Vec<u32> = f
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| bytemuck::pod_read_unaligned(bytes))
            .collect();
        Ok(Stats {
            source_triangles: self.source_triangles,
            clusters_drawn: args[12],
            software_triangles: feedback[self.cluster_count as usize],
            hardware_triangles: feedback[self.cluster_count as usize + 1],
            overflow: args[15],
            instances_frustum_culled: args[16],
            instances_occluded_previous: args[17],
            instances_occluded_current: args[18],
            clusters_culled: args[19],
            pages_resident: self.residency.pool.len() as u32,
            page_pool_bytes: u64::from(self.residency.capacity) * 2048,
            resident_floor: self.residency.floor,
        })
    }
    pub(super) fn read_image(
        &self,
        gpu: &mut VulkanDevice,
        image: Image,
        stride: u64,
    ) -> Result<Vec<u8>> {
        let read = gpu.create_buffer(
            u64::from(self.size[0]) * u64::from(self.size[1]) * stride,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut r = gpu.begin(Queue::Graphics);
        r.barriers([barrier(
            Resource::Image(image),
            Access::ShaderRead,
            Access::CopySource,
        )]);
        r.push(Command::ReadImage {
            source: image,
            destination: read,
        });
        r.barriers([
            barrier(
                Resource::Image(image),
                Access::CopySource,
                Access::ShaderRead,
            ),
            barrier(
                Resource::Buffer(read),
                Access::CopyDestination,
                Access::HostRead,
            ),
        ]);
        let p = gpu.submit(r.finish(), &[])?;
        gpu.resolve(p)?;
        let data = gpu.read(read)?;
        gpu.retire(Resource::Buffer(read))?;
        gpu.collect()?;
        Ok(data)
    }
    pub(super) fn read_buffer(
        &self,
        gpu: &mut VulkanDevice,
        source: Buffer,
        state: Access,
        size: usize,
    ) -> Result<Vec<u8>> {
        let read = gpu.create_buffer(size as u64, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        let mut r = gpu.begin(Queue::Graphics);
        r.barriers([barrier(Resource::Buffer(source), state, Access::CopySource)]);
        r.push(Command::CopyBuffer {
            source,
            destination: read,
            size: size as u64,
        });
        r.barriers([
            barrier(Resource::Buffer(source), Access::CopySource, state),
            barrier(
                Resource::Buffer(read),
                Access::CopyDestination,
                Access::HostRead,
            ),
        ]);
        let p = gpu.submit(r.finish(), &[])?;
        gpu.resolve(p)?;
        let data = gpu.read(read)?;
        gpu.retire(Resource::Buffer(read))?;
        gpu.collect()?;
        Ok(data)
    }
    /// Retires view resources after the host has completed its last use.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        for b in self.buffers.into_iter().chain([
            self.frame,
            self.residency.table,
            self.residency.readback,
            self.residency.upload,
        ]) {
            gpu.retire(Resource::Buffer(b))?;
        }
        for i in self.images.into_iter().chain([self.depth]) {
            gpu.retire(Resource::Image(i))?;
        }
        for p in self.pipelines.into_values() {
            gpu.retire_pipeline(p)?;
        }
        gpu.collect()?;
        Ok(())
    }
}
fn barrier(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
