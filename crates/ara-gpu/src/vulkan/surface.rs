use super::*;
use crate::descriptors::*;

impl BindlessHeap for VulkanDevice {
    fn insert_acceleration_structure(
        &mut self,
        structure: AccelerationStructure,
    ) -> Result<HeapIndex> {
        self.bind_acceleration_structure(structure)
    }
    fn insert_image(&mut self, image: Image) -> Result<HeapIndex> {
        self.sampled_image(image, false)
    }
    fn insert_buffer(&mut self, buffer: Buffer) -> Result<HeapIndex> {
        self.storage_buffer(buffer)
    }
    fn insert_storage_image(&mut self, image: Image) -> Result<HeapIndex> {
        self.storage_image(image)
    }
}
impl VulkanDevice {
    pub(super) fn portable_buffer(&mut self, desc: &BufferDesc) -> Result<Buffer> {
        self.create_buffer(desc.size, desc.vk_usage(), desc.host_visible)
    }
    pub(super) fn portable_image(&mut self, desc: &ImageDesc) -> Result<Image> {
        self.create_image(
            desc.size[0],
            desc.size[1],
            desc.format.vk(),
            desc.vk_usage(),
        )
    }
}
