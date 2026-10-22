use ash::vk::TaggedStructure;
mod resources;
pub use resources::{Buffer, Image, image_barrier};

use anyhow::{Context, Result, bail, ensure};
use ash::{Entry, vk};
use serde::Serialize;
use std::{
    ffi::{CStr, c_void},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Validation callback counters retained through Vulkan teardown.
#[derive(Default)]
pub struct Validation {
    pub errors: AtomicUsize,
    pub warnings: AtomicUsize,
    pub messages: Mutex<Vec<String>>,
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _kind: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user: *mut c_void,
) -> vk::Bool32 {
    // SAFETY: Vulkan calls with a live callback data pointer and the boxed Validation
    // remains at a stable address until after instance/messenger destruction.
    unsafe {
        let state = &*(user as *const Validation);
        let message = CStr::from_ptr((*data).p_message)
            .to_string_lossy()
            .into_owned();
        if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
            state.errors.fetch_add(1, Ordering::Relaxed);
        } else {
            state.warnings.fetch_add(1, Ordering::Relaxed);
        }
        eprintln!("VALIDATION {severity:?}: {message}");
        if let Ok(mut messages) = state.messages.lock() {
            messages.push(message);
        }
    }
    vk::FALSE
}

/// Driver identity and queue capabilities recorded with every measurement.
#[derive(Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub api_version: String,
    pub driver_version: u32,
    pub graphics_family: u32,
    pub compute_family: u32,
    pub graphics_queue_flags: String,
    pub compute_queue_flags: String,
    pub descriptor_heap_available: bool,
    pub timestamp_period_ns: f32,
    pub timestamp_valid_bits: [u32; 2],
    pub pipeline_cache_uuid: Vec<u8>,
}

/// Owns the instance, queues, allocations and validation callback lifetime.
pub struct Gpu {
    pub _entry: Entry,
    pub instance: ash::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub graphics: vk::Queue,
    pub compute: vk::Queue,
    pub graphics_pool: vk::CommandPool,
    pub compute_pool: vk::CommandPool,
    pub acceleration: ash::khr::acceleration_structure::Device,
    pub info: DeviceInfo,
    pub validation: Box<Validation>,
    pub scratch_alignment: u64,
    memory: vk::PhysicalDeviceMemoryProperties,
    debug: ash::ext::debug_utils::Instance,
    messenger: vk::DebugUtilsMessengerEXT,
    buffers: Vec<Buffer>,
    images: Vec<Image>,
    structures: Vec<vk::AccelerationStructureKHR>,
}

impl Gpu {
    /// Enables synchronization validation explicitly; failure to find it is fatal.
    pub fn new(window: Option<&crate::window::Window>) -> Result<Self> {
        let mut validation = Box::<Validation>::default();
        // SAFETY: All create-info slices live through their calls; the entry,
        // instance, device and callback userdata are retained by Gpu.
        unsafe {
            let entry = Entry::load()?;
            let layers = entry.enumerate_instance_layer_properties()?;
            ensure!(
                layers.iter().any(
                    |l| CStr::from_ptr(l.layer_name.as_ptr()) == c"VK_LAYER_KHRONOS_validation"
                ),
                "Khronos validation layer unavailable"
            );
            let mut extensions = vec![
                ash::ext::debug_utils::NAME.as_ptr(),
                ash::ext::validation_features::NAME.as_ptr(),
            ];
            if let Some(window) = window {
                extensions.extend(window.extensions()?);
            }
            let layer_names = [c"VK_LAYER_KHRONOS_validation".as_ptr()];
            let enabled = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
            let mut features =
                vk::ValidationFeaturesEXT::default().enabled_validation_features(&enabled);
            let mut debug_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
                .message_severity(
                    vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                        | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                )
                .message_type(
                    vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                        | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                        | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
                )
                .pfn_user_callback(Some(debug_callback))
                .user_data((&mut *validation as *mut Validation).cast());
            let app = vk::ApplicationInfo::default()
                .application_name(c"Ara S1")
                .api_version(vk::API_VERSION_1_3);
            let instance = entry.create_instance(
                &vk::InstanceCreateInfo::default()
                    .application_info(&app)
                    .enabled_extension_names(&extensions)
                    .enabled_layer_names(&layer_names)
                    .push(&mut features)
                    .push(&mut debug_info),
                None,
            )?;
            let debug = ash::ext::debug_utils::Instance::load(&entry, &instance);
            let messenger = debug.create_debug_utils_messenger(&debug_info, None)?;
            let physical = instance
                .enumerate_physical_devices()?
                .into_iter()
                .find(|p| {
                    instance.get_physical_device_properties(*p).device_type
                        == vk::PhysicalDeviceType::DISCRETE_GPU
                })
                .context("No discrete GPU")?;
            let properties = instance.get_physical_device_properties(physical);
            ensure!(
                properties.api_version >= vk::API_VERSION_1_3,
                "Vulkan 1.3 required"
            );
            let families = instance.get_physical_device_queue_family_properties(physical);
            let graphics_family = families
                .iter()
                .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .context("No graphics queue")? as u32;
            let compute_family = families
                .iter()
                .position(|f| {
                    f.queue_flags.contains(vk::QueueFlags::COMPUTE)
                        && !f.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                })
                .context("A separate compute-only family is required by this spike")?
                as u32;
            ensure!(
                families[graphics_family as usize].timestamp_valid_bits > 0
                    && families[compute_family as usize].timestamp_valid_bits > 0,
                "Both queues need timestamps"
            );
            let available = instance.enumerate_device_extension_properties(physical)?;
            let has_extension = |name: &CStr| {
                available
                    .iter()
                    .any(|e| CStr::from_ptr(e.extension_name.as_ptr()) == name)
            };
            let mut device_extensions = vec![
                ash::khr::acceleration_structure::NAME.as_ptr(),
                ash::khr::ray_query::NAME.as_ptr(),
                ash::khr::deferred_host_operations::NAME.as_ptr(),
            ];
            if window.is_some() {
                device_extensions.push(ash::khr::swapchain::NAME.as_ptr());
            }
            for &extension in &device_extensions {
                ensure!(
                    has_extension(CStr::from_ptr(extension)),
                    "Missing {:?}",
                    CStr::from_ptr(extension)
                );
            }
            let mut v12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut v13 = vk::PhysicalDeviceVulkan13Features::default();
            let mut acceleration_features =
                vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
            let mut ray = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
            let mut supported = vk::PhysicalDeviceFeatures2::default()
                .push(&mut v12)
                .push(&mut v13)
                .push(&mut acceleration_features)
                .push(&mut ray);
            instance.get_physical_device_features2(physical, &mut supported);
            ensure!(
                v12.timeline_semaphore != 0
                    && v12.buffer_device_address != 0
                    && v12.descriptor_indexing != 0
                    && v12.shader_sampled_image_array_non_uniform_indexing != 0
                    && v12.shader_storage_buffer_array_non_uniform_indexing != 0
                    && v12.descriptor_binding_sampled_image_update_after_bind != 0
                    && v12.descriptor_binding_storage_buffer_update_after_bind != 0
                    && v12.descriptor_binding_partially_bound != 0
                    && v13.dynamic_rendering != 0
                    && v13.synchronization2 != 0
                    && acceleration_features.acceleration_structure != 0
                    && ray.ray_query != 0,
                "Missing required bindless/RT/sync features"
            );
            let priorities = [1.0];
            let queues = [
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(graphics_family)
                    .queue_priorities(&priorities),
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(compute_family)
                    .queue_priorities(&priorities),
            ];
            let mut v12 = vk::PhysicalDeviceVulkan12Features::default()
                .timeline_semaphore(true)
                .buffer_device_address(true)
                .descriptor_indexing(true)
                .shader_sampled_image_array_non_uniform_indexing(true)
                .shader_storage_buffer_array_non_uniform_indexing(true)
                .descriptor_binding_sampled_image_update_after_bind(true)
                .descriptor_binding_storage_buffer_update_after_bind(true)
                .descriptor_binding_partially_bound(true);
            let mut v13 = vk::PhysicalDeviceVulkan13Features::default()
                .dynamic_rendering(true)
                .synchronization2(true);
            let mut acceleration_features =
                vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
                    .acceleration_structure(true);
            let mut ray = vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true);
            let core = vk::PhysicalDeviceFeatures::default()
                .fragment_stores_and_atomics(true)
                .shader_storage_buffer_array_dynamic_indexing(true)
                .shader_sampled_image_array_dynamic_indexing(true);
            let device = instance.create_device(
                physical,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(&queues)
                    .enabled_extension_names(&device_extensions)
                    .enabled_features(&core)
                    .push(&mut v12)
                    .push(&mut v13)
                    .push(&mut acceleration_features)
                    .push(&mut ray),
                None,
            )?;
            let graphics = device.get_device_queue(graphics_family, 0);
            let compute = device.get_device_queue(compute_family, 0);
            let graphics_pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(graphics_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let compute_pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(compute_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let acceleration = ash::khr::acceleration_structure::Device::load(&instance, &device);
            let memory = instance.get_physical_device_memory_properties(physical);
            let mut as_properties = vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
            instance.get_physical_device_properties2(
                physical,
                &mut vk::PhysicalDeviceProperties2::default().push(&mut as_properties),
            );
            let info = DeviceInfo {
                name: CStr::from_ptr(properties.device_name.as_ptr())
                    .to_string_lossy()
                    .into_owned(),
                api_version: format!(
                    "{}.{}.{}",
                    vk::api_version_major(properties.api_version),
                    vk::api_version_minor(properties.api_version),
                    vk::api_version_patch(properties.api_version)
                ),
                driver_version: properties.driver_version,
                graphics_family,
                compute_family,
                graphics_queue_flags: format!(
                    "{:?}",
                    families[graphics_family as usize].queue_flags
                ),
                compute_queue_flags: format!("{:?}", families[compute_family as usize].queue_flags),
                descriptor_heap_available: has_extension(c"VK_EXT_descriptor_heap"),
                timestamp_period_ns: properties.limits.timestamp_period,
                timestamp_valid_bits: [
                    families[graphics_family as usize].timestamp_valid_bits,
                    families[compute_family as usize].timestamp_valid_bits,
                ],
                pipeline_cache_uuid: properties.pipeline_cache_uuid.to_vec(),
            };
            println!(
                "{}; graphics family {graphics_family}; compute-only family {compute_family}",
                info.name
            );
            Ok(Self {
                _entry: entry,
                instance,
                physical,
                device,
                graphics,
                compute,
                graphics_pool,
                compute_pool,
                acceleration,
                info,
                validation,
                memory,
                debug,
                messenger,
                scratch_alignment: u64::from(
                    as_properties.min_acceleration_structure_scratch_offset_alignment,
                ),
                buffers: vec![],
                images: vec![],
                structures: vec![],
            })
        }
    }

    /// Allocate one primary command buffer for the selected queue family.
    pub fn commands(&self, compute: bool) -> Result<vk::CommandBuffer> {
        // SAFETY: Pools belong to their respective queue families and outlive commands.
        unsafe {
            Ok(self.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(if compute {
                        self.compute_pool
                    } else {
                        self.graphics_pool
                    })
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0])
        }
    }

    /// Reset a completed command buffer and begin its next recording.
    pub fn begin(&self, cmd: vk::CommandBuffer) -> Result<()> {
        // SAFETY: All callsites wait for completion before resetting command buffers.
        unsafe {
            self.device
                .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            self.device
                .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())?;
        }
        Ok(())
    }

    /// A startup-only blocking submit; frame work uses submit2 with timelines.
    pub fn immediate(&self, record: impl FnOnce(vk::CommandBuffer)) -> Result<()> {
        let cmd = self.commands(false)?;
        self.begin(cmd)?;
        record(cmd);
        // SAFETY: Command is complete, belongs to graphics, and is freed after idle.
        unsafe {
            self.device.end_command_buffer(cmd)?;
            let buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
            self.device.queue_submit2(
                self.graphics,
                &[vk::SubmitInfo2::default().command_buffer_infos(&buffers)],
                vk::Fence::null(),
            )?;
            self.device.queue_wait_idle(self.graphics)?;
            self.device.free_command_buffers(self.graphics_pool, &[cmd]);
        }
        Ok(())
    }

    /// Fail immediately if validation has reported any error or hazard.
    pub fn assert_clean(&self) -> Result<()> {
        let errors = self.validation.errors.load(Ordering::Relaxed);
        if errors > 0 {
            bail!("{errors} validation errors (including synchronization hazards)");
        }
        Ok(())
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: Last owner; wait for all queues before destroying children, then parents.
        unsafe {
            let _ = self.device.device_wait_idle();
            for structure in &self.structures {
                self.acceleration
                    .destroy_acceleration_structure(*structure, None);
            }
            for image in &self.images {
                self.device.destroy_image_view(image.view, None);
                self.device.destroy_image(image.handle, None);
                self.device.free_memory(image.memory, None);
            }
            for buffer in &self.buffers {
                self.device.destroy_buffer(buffer.handle, None);
                self.device.free_memory(buffer.memory, None);
            }
            self.device.destroy_command_pool(self.compute_pool, None);
            self.device.destroy_command_pool(self.graphics_pool, None);
            self.device.destroy_device(None);
            self.debug
                .destroy_debug_utils_messenger(self.messenger, None);
            self.instance.destroy_instance(None);
        }
    }
}
