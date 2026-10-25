//! Failure-path guards for partially constructed native objects.
use ash::{Device, Instance, vk};

#[derive(Default)]
pub(super) struct Initialization {
    pub instance: Option<Instance>,
    pub debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub device: Option<Device>,
    pub pools: Vec<vk::CommandPool>,
}
impl Initialization {
    pub fn new(instance: Instance) -> Self {
        Self {
            instance: Some(instance),
            debug: None,
            device: None,
            pools: Vec::new(),
        }
    }
    pub fn disarm(&mut self) {
        self.instance = None;
        self.debug = None;
        self.device = None;
        self.pools.clear();
    }
}
impl Drop for Initialization {
    fn drop(&mut self) {
        // SAFETY: Constructor has not submitted work; children are destroyed before parents.
        unsafe {
            if let Some(device) = &self.device {
                for pool in &self.pools {
                    device.destroy_command_pool(*pool, None);
                }
                device.destroy_device(None);
            }
            if let Some((debug, messenger)) = &self.debug {
                debug.destroy_debug_utils_messenger(*messenger, None);
            }
            if let Some(instance) = &self.instance {
                instance.destroy_instance(None);
            }
        }
    }
}
pub(super) enum Object {
    SetLayout(vk::DescriptorSetLayout),
    Pool(vk::DescriptorPool),
    Sampler(vk::Sampler),
    Layout(vk::PipelineLayout),
    Cache(vk::PipelineCache),
    Semaphore(vk::Semaphore),
    Fence(vk::Fence),
    Query(vk::QueryPool),
    Commands(vk::CommandPool, vk::CommandBuffer),
}
pub(super) struct Objects<'a> {
    device: &'a Device,
    objects: Vec<Object>,
}
impl<'a> Objects<'a> {
    pub fn new(device: &'a Device) -> Self {
        Self {
            device,
            objects: Vec::new(),
        }
    }
    pub fn push(&mut self, object: Object) {
        self.objects.push(object);
    }
    /// Transfers one query pool to completed-object storage without destroying it.
    pub fn take_query(&mut self, pool: vk::QueryPool) -> Option<vk::QueryPool> {
        let index = self
            .objects
            .iter()
            .position(|object| matches!(object, Object::Query(handle) if *handle == pool))?;
        self.objects.swap_remove(index);
        Some(pool)
    }
    pub fn disarm(mut self) {
        self.objects.clear();
    }
}
impl Drop for Objects<'_> {
    fn drop(&mut self) {
        // SAFETY: Construction objects were never submitted; resolved query,
        // command and readback objects have completed all GPU references.
        unsafe {
            for object in self.objects.drain(..).rev() {
                match object {
                    Object::SetLayout(h) => self.device.destroy_descriptor_set_layout(h, None),
                    Object::Pool(h) => self.device.destroy_descriptor_pool(h, None),
                    Object::Sampler(h) => self.device.destroy_sampler(h, None),
                    Object::Layout(h) => self.device.destroy_pipeline_layout(h, None),
                    Object::Cache(h) => self.device.destroy_pipeline_cache(h, None),
                    Object::Semaphore(h) => self.device.destroy_semaphore(h, None),
                    Object::Fence(h) => self.device.destroy_fence(h, None),
                    Object::Query(h) => self.device.destroy_query_pool(h, None),
                    Object::Commands(pool, h) => self.device.free_command_buffers(pool, &[h]),
                }
            }
        }
    }
}
