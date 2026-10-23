use super::*;
impl VirtualShadows {
    /// Completed scalar visibility/residency channels for reference comparisons.
    pub fn samples(
        &self,
        gpu: &mut VulkanDevice,
        geometry: &GeometryView,
    ) -> Result<Vec<[f32; 4]>> {
        Ok(geometry
            .read_image(gpu, self.mask, 16)?
            .as_chunks::<16>()
            .0
            .iter()
            .map(|b| bytemuck::pod_read_unaligned(b))
            .collect())
    }
    /// Reads actual GPU cache/raster counters after completed rendering.
    pub fn stats(&self, gpu: &mut VulkanDevice, geometry: &GeometryView) -> Result<ShadowStats> {
        let a = geometry.read_buffer(gpu, self.buffers[4], self.previous[4].access, 128)?;
        let b = geometry.read_buffer(gpu, self.buffers[7], self.previous[7].access, 128)?;
        let a: Vec<u32> = a
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        let b: Vec<u32> = b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        Ok(ShadowStats {
            dirty_pages: a[3],
            missing_pages: a[5],
            cache_hits: a[7],
            clusters_drawn: b[1],
            cluster_overflow: b[4],
            page_pool_bytes: u64::from(self.capacity) * 128 * 128 * 4,
        })
    }
    /// Saves scalar PCF visibility as an inspectable diagnostic PNG.
    pub fn capture(
        &self,
        gpu: &mut VulkanDevice,
        geometry: &GeometryView,
        path: &Path,
    ) -> Result<()> {
        let bytes = geometry.read_image(gpu, self.mask, 16)?;
        let pixels: Vec<u8> = bytes
            .as_chunks::<16>()
            .0
            .iter()
            .flat_map(|b| {
                let value = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                let c = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                [c, c, c, 255]
            })
            .collect();
        image::save_buffer(
            path,
            &pixels,
            self.size[0],
            self.size[1],
            image::ColorType::Rgba8,
        )?;
        Ok(())
    }
    /// Retires persistent resources after the host has completed their last use.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        for b in self.buffers.into_iter().chain([self.frame]) {
            gpu.retire(Resource::Buffer(b))?;
        }
        gpu.retire(Resource::Image(self.mask))?;
        for p in self.pipelines.into_values() {
            gpu.retire_pipeline(p)?;
        }
        gpu.collect()?;
        Ok(())
    }
}
