use super::context::BufferAllocation;
use super::{Context, Image};
use crate::{GpuError, HeapIndex, Result};
use ash::vk::{self, TaggedStructure};

/// Currently reserved bindless slots, including resources awaiting retirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescriptorUsage {
    /// Sampled image descriptors reserved by live or retiring images.
    pub sampled_images: usize,
    /// Storage buffer descriptors reserved by live or retiring buffers.
    pub storage_buffers: usize,
    /// Storage image descriptors reserved by live or retiring images.
    pub storage_images: usize,
    /// Acceleration descriptors reserved by live or retiring structures.
    pub acceleration_structures: usize,
}

/// Anisotropic filtering ceiling of the material sampler (`samplers[0]`, wrap/wrap).
/// Native material passes sample it with explicit gradients (`SampleGrad`); its
/// explicit-LOD readers (RT hits, GI material fetches, mip tails) filter through it too,
/// where NVIDIA blends over the first half of each LOD step only (up to half a mip softer
/// than trilinear). The Metal backend's matching sampler uses the same value.
const MATERIAL_ANISOTROPY: f32 = 16.0;
/// Storage-image slots: the binding-3 array size every cooked shader declares
/// (`images[128]`), part of each pipeline's binding-layout hash.
const STORAGE_IMAGES: u32 = 128;

/// The material sampler's `maxAnisotropy`: [`MATERIAL_ANISOTROPY`] clamped to the
/// device limit, or 1.0 (isotropic) where `samplerAnisotropy` is unsupported, which is
/// exactly when `context/create.rs` leaves the feature disabled.
fn material_anisotropy(gpu: &Context) -> f32 {
    // SAFETY: the instance and physical device outlive `gpu`; both are read-only queries.
    let (features, limits) = unsafe {
        (
            gpu.instance.get_physical_device_features(gpu.physical),
            gpu.instance
                .get_physical_device_properties(gpu.physical)
                .limits,
        )
    };
    if features.sampler_anisotropy == 0 {
        1.0
    } else {
        MATERIAL_ANISOTROPY.min(limits.max_sampler_anisotropy)
    }
}

pub(super) struct Heap {
    pub layout: vk::PipelineLayout,
    pub set: vk::DescriptorSet,
    set_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    samplers: Vec<vk::Sampler>,
    sampler_descs: Vec<crate::sampler::SamplerDesc>,
    comparison_samplers: Vec<vk::Sampler>,
    comparison_descs: Vec<crate::sampler::SamplerDesc>,
    free_acceleration: Vec<u32>,
    free_images: Vec<u32>,
    free_buffers: Vec<u32>,
    free_storage: Vec<u32>,
    storage: Vec<(Image, vk::ImageView, HeapIndex)>,
}
impl Heap {
    pub fn sampler(
        &mut self,
        gpu: &Context,
        desc: &crate::sampler::SamplerDesc,
    ) -> Result<HeapIndex> {
        use crate::sampler::{AddressMode as A, Filter as F};
        desc.validate()?;
        let comparison = desc.compare.is_some();
        let (samplers, descs) = if comparison {
            (&mut self.comparison_samplers, &mut self.comparison_descs)
        } else {
            (&mut self.samplers, &mut self.sampler_descs)
        };
        if let Some(index) = descs.iter().position(|d| d == desc) {
            return Ok(HeapIndex {
                index: index as u32,
            });
        }
        if samplers.len() >= crate::sampler::SAMPLER_CAPACITY as usize {
            return Err(GpuError::Contract("Sampler heap exhausted".into()));
        }
        let filter = |f| match f {
            F::Nearest => vk::Filter::NEAREST,
            F::Linear => vk::Filter::LINEAR,
        };
        let address = |a| match a {
            A::Repeat => vk::SamplerAddressMode::REPEAT,
            A::MirrorRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
            A::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        };
        let anisotropy = (desc.anisotropy as f32).min(material_anisotropy(gpu));
        // SAFETY: validated immutable descriptor; the device retains this sampler through teardown.
        let sampler = unsafe {
            gpu.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .min_filter(filter(desc.min))
                    .mag_filter(filter(desc.mag))
                    .mipmap_mode(match desc.mip {
                        F::Nearest => vk::SamplerMipmapMode::NEAREST,
                        F::Linear => vk::SamplerMipmapMode::LINEAR,
                    })
                    .address_mode_u(address(desc.address[0]))
                    .address_mode_v(address(desc.address[1]))
                    .address_mode_w(address(desc.address[2]))
                    .compare_enable(desc.compare.is_some())
                    .compare_op(desc.compare.unwrap_or_default().vk())
                    .anisotropy_enable(anisotropy > 1.0)
                    .max_anisotropy(anisotropy)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .map_err(anyhow::Error::from)?;
        let index = samplers.len() as u32;
        let info = [vk::DescriptorImageInfo::default().sampler(sampler)];
        // SAFETY: a fresh partially bound slot is never read by earlier submissions.
        unsafe {
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(if comparison { 6 } else { 2 })
                    .dst_array_element(index)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&info)],
                &[],
            );
        }
        samplers.push(sampler);
        descs.push(*desc);
        Ok(HeapIndex { index })
    }
    pub fn usage(&self, gpu: &Context) -> DescriptorUsage {
        DescriptorUsage {
            sampled_images: gpu.info.sampled_image_capacity as usize - self.free_images.len(),
            storage_buffers: gpu.info.storage_buffer_capacity as usize - self.free_buffers.len(),
            storage_images: STORAGE_IMAGES as usize - self.free_storage.len(),
            acceleration_structures: if gpu.info.ray_tracing { 16 } else { 0 }
                - self.free_acceleration.len(),
        }
    }
    pub fn new(gpu: &Context) -> Result<Self> {
        // SAFETY: The versioned binding ABI is validated at pipeline creation;
        // create-info backing slices remain live through their native calls.
        unsafe {
            let mut guard = super::guard::Objects::new(&gpu.device);
            let flags = vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::PARTIALLY_BOUND;
            let mut binding_flags = vec![flags, flags, flags, flags];
            let mut bindings = vec![
                (0, vk::DescriptorType::SAMPLED_IMAGE, 128),
                (
                    1,
                    vk::DescriptorType::STORAGE_BUFFER,
                    gpu.info.storage_buffer_capacity,
                ),
                (
                    2,
                    vk::DescriptorType::SAMPLER,
                    crate::sampler::SAMPLER_CAPACITY,
                ),
                (3, vk::DescriptorType::STORAGE_IMAGE, STORAGE_IMAGES),
            ];
            if gpu.info.ray_tracing {
                bindings.push((4, vk::DescriptorType::ACCELERATION_STRUCTURE_KHR, 16));
                binding_flags.push(flags);
            }
            // Runtime shader arrays use the capacity selected for this device;
            // every set has that capacity, so variable descriptor counts are unnecessary.
            bindings.push((
                5,
                vk::DescriptorType::SAMPLED_IMAGE,
                gpu.info.sampled_image_capacity,
            ));
            binding_flags.push(flags);
            bindings.push((
                6,
                vk::DescriptorType::SAMPLER,
                crate::sampler::SAMPLER_CAPACITY,
            ));
            binding_flags.push(flags);
            let bindings = bindings
                .into_iter()
                .map(|(binding, ty, count)| {
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(binding)
                        .descriptor_type(ty)
                        .descriptor_count(count)
                        .stage_flags(vk::ShaderStageFlags::ALL)
                })
                .collect::<Vec<_>>();
            let mut flag_info = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default()
                .binding_flags(&binding_flags);
            let set_layout = gpu
                .device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default()
                        .bindings(&bindings)
                        .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
                        .push(&mut flag_info),
                    None,
                )
                .map_err(anyhow::Error::from)?;
            guard.push(super::guard::Object::SetLayout(set_layout));
            let sizes = bindings
                .iter()
                .map(|b| vk::DescriptorPoolSize {
                    ty: b.descriptor_type,
                    descriptor_count: b.descriptor_count,
                })
                .collect::<Vec<_>>();
            let pool = gpu
                .device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes)
                        .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND),
                    None,
                )
                .map_err(anyhow::Error::from)?;
            guard.push(super::guard::Object::Pool(pool));
            let set = gpu
                .device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&[set_layout]),
                )
                .map_err(anyhow::Error::from)?[0];
            let mut samplers = [vk::Sampler::null(); 3];
            let material = material_anisotropy(gpu);
            for (index, sampler) in samplers.iter_mut().enumerate() {
                // Only the material sampler (index 0) is anisotropic; the environment
                // and temporal samplers stay trilinear.
                let anisotropy = if index == 0 { material } else { 1.0 };
                *sampler = gpu
                    .device
                    .create_sampler(
                        &vk::SamplerCreateInfo::default()
                            .mag_filter(vk::Filter::LINEAR)
                            .min_filter(vk::Filter::LINEAR)
                            .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                            .anisotropy_enable(anisotropy > 1.0)
                            .max_anisotropy(anisotropy)
                            .max_lod(vk::LOD_CLAMP_NONE)
                            .address_mode_u(if index == 2 {
                                vk::SamplerAddressMode::CLAMP_TO_EDGE
                            } else {
                                vk::SamplerAddressMode::REPEAT
                            })
                            .address_mode_v(if index == 0 {
                                vk::SamplerAddressMode::REPEAT
                            } else {
                                vk::SamplerAddressMode::CLAMP_TO_EDGE
                            })
                            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                        None,
                    )
                    .map_err(anyhow::Error::from)?;
                guard.push(super::guard::Object::Sampler(*sampler));
            }
            let infos = samplers.map(|sampler| vk::DescriptorImageInfo::default().sampler(sampler));
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&infos)],
                &[],
            );
            let constants = [vk::PushConstantRange::default()
                .size(32)
                .stage_flags(vk::ShaderStageFlags::ALL)];
            let layout = gpu
                .device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&[set_layout])
                        .push_constant_ranges(&constants),
                    None,
                )
                .map_err(anyhow::Error::from)?;
            guard.push(super::guard::Object::Layout(layout));
            guard.disarm();
            Ok(Self {
                layout,
                set,
                set_layout,
                pool,
                samplers: samplers.to_vec(),
                sampler_descs: crate::sampler::SamplerDesc::defaults().to_vec(),
                comparison_samplers: Vec::new(),
                comparison_descs: Vec::new(),
                free_acceleration: if gpu.info.ray_tracing {
                    (0..16).rev().collect()
                } else {
                    Vec::new()
                },
                free_images: (0..gpu.info.sampled_image_capacity).rev().collect(),
                free_buffers: (0..gpu.info.storage_buffer_capacity).rev().collect(),
                free_storage: (0..STORAGE_IMAGES).rev().collect(),
                storage: Vec::new(),
            })
        }
    }
    pub fn image(&mut self, gpu: &Context, view: vk::ImageView, _depth: bool) -> Result<HeapIndex> {
        let index = self
            .free_images
            .pop()
            .ok_or_else(|| GpuError::Contract("Sampled heap exhausted".into()))?;
        let info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        // SAFETY: Fresh UAB slot cannot have a pending reader. Resource lifetime follows retirement.
        unsafe {
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(5)
                    .dst_array_element(index)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&info),
                // A compatibility alias only, never the engine scene's limit.
                vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(0)
                    .dst_array_element(index)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&info),
            ];
            gpu.device
                .update_descriptor_sets(&writes[..if index < 128 { 2 } else { 1 }], &[]);
        }
        Ok(HeapIndex { index })
    }
    pub fn buffer(&mut self, gpu: &Context, buffer: BufferAllocation) -> Result<HeapIndex> {
        let index = self.free_buffers.pop().ok_or_else(|| {
            GpuError::Contract(format!(
                "Buffer heap exhausted ({} slots, including pending retirement)",
                gpu.info.storage_buffer_capacity
            ))
        })?;
        let info = [vk::DescriptorBufferInfo::default()
            .buffer(buffer.handle)
            .range(buffer.size)];
        // SAFETY: Fresh descriptor slot with live storage retained through all queue uses.
        unsafe {
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(1)
                    .dst_array_element(index)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&info)],
                &[],
            );
        }
        Ok(HeapIndex { index })
    }
    /// Whether every storage-image slot is taken.
    pub fn storage_full(&self) -> bool {
        self.free_storage.is_empty()
    }
    /// Whether `view` holds a storage-image slot.
    pub fn has_storage(&self, view: vk::ImageView) -> bool {
        self.storage.iter().any(|(_, v, _)| *v == view)
    }
    pub fn storage_image(
        &mut self,
        gpu: &Context,
        image: Image,
        view: vk::ImageView,
    ) -> Result<HeapIndex> {
        if let Some((_, _, index)) = self.storage.iter().find(|(handle, _, _)| *handle == image) {
            return Ok(*index);
        }
        let index = HeapIndex {
            index: self
                .free_storage
                .pop()
                .ok_or_else(|| GpuError::Contract("Storage-image heap exhausted".into()))?,
        };
        let info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        // SAFETY: Slot is fresh; callers transition the resource before accessing it.
        unsafe {
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(3)
                    .dst_array_element(index.index)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(&info)],
                &[],
            );
        }
        self.storage.push((image, view, index));
        Ok(index)
    }
    pub fn view(&mut self, gpu: &Context, view: vk::ImageView, storage: bool) -> Result<HeapIndex> {
        if !storage {
            return self.image(gpu, view, false);
        }
        let index = self
            .free_storage
            .pop()
            .ok_or_else(|| GpuError::Contract("Storage-image heap exhausted".into()))?;
        let info = [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::GENERAL)];
        // SAFETY: fresh slot and view retained with the parent allocation.
        unsafe {
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(3)
                    .dst_array_element(index)
                    .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                    .image_info(&info)],
                &[],
            );
        }
        Ok(HeapIndex { index })
    }
    pub fn retire_view(&mut self, slot: HeapIndex, storage: bool) {
        if storage {
            self.free_storage.push(slot.index);
        } else {
            self.free_images.push(slot.index);
        }
    }
    pub fn acceleration(
        &mut self,
        gpu: &Context,
        structure: vk::AccelerationStructureKHR,
    ) -> Result<HeapIndex> {
        let index = self
            .free_acceleration
            .pop()
            .ok_or_else(|| GpuError::Contract("Acceleration heap exhausted".into()))?;
        let structures = [structure];
        let mut info = vk::WriteDescriptorSetAccelerationStructureKHR::default()
            .acceleration_structures(&structures);
        // SAFETY: Fresh UAB slot and live TLAS; all queue uses retain its BLAS dependencies.
        unsafe {
            gpu.device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(self.set)
                    .dst_binding(4)
                    .dst_array_element(index)
                    .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                    .descriptor_count(1)
                    .push(&mut info)],
                &[],
            );
        }
        Ok(HeapIndex { index })
    }
    pub fn retire_acceleration(&mut self, index: Option<HeapIndex>) {
        if let Some(index) = index {
            self.free_acceleration.push(index.index);
        }
    }
    pub fn retire_buffer(&mut self, index: Option<HeapIndex>) {
        if let Some(index) = index {
            self.free_buffers.push(index.index);
        }
    }
    pub fn retire_image(&mut self, view: vk::ImageView, index: Option<HeapIndex>) {
        if let Some(index) = index {
            self.free_images.push(index.index);
        }
        if let Some(i) = self.storage.iter().position(|(_, v, _)| *v == view) {
            self.free_storage.push(self.storage.swap_remove(i).2.index);
        }
    }
    pub fn destroy(&self, gpu: &Context) {
        // SAFETY: Device idle; descriptor sets and layouts are no longer referenced.
        unsafe {
            gpu.device.destroy_pipeline_layout(self.layout, None);
            gpu.device.destroy_descriptor_pool(self.pool, None);
            gpu.device
                .destroy_descriptor_set_layout(self.set_layout, None);
            for sampler in self.samplers.iter().chain(&self.comparison_samplers) {
                gpu.device.destroy_sampler(*sampler, None);
            }
        }
    }
}
