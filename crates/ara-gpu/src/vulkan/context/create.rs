use super::*;
use anyhow::{Context, ensure};
use ash::vk::TaggedStructure;

impl Gpu {
    /// Enables synchronization validation explicitly; failure to find it is fatal.
    pub fn new(window_extensions: &[std::ffi::CString]) -> Result<Self> {
        let mut validation = Box::<Validation>::default();
        // SAFETY: All create-info slices live through their calls; the entry,
        // instance, device and callback userdata are retained by Gpu.
        unsafe {
            let entry = Entry::load()?;
            let layers = entry.enumerate_instance_layer_properties()?;
            ensure!(
                !cfg!(debug_assertions)
                    || layers
                        .iter()
                        .any(|l| CStr::from_ptr(l.layer_name.as_ptr())
                            == c"VK_LAYER_KHRONOS_validation"),
                "Khronos validation layer unavailable"
            );
            let mut extensions = vec![ash::ext::debug_utils::NAME.as_ptr()];
            if cfg!(debug_assertions) {
                extensions.push(ash::ext::validation_features::NAME.as_ptr());
            }
            extensions.extend(window_extensions.iter().map(|s| s.as_ptr()));
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
                .application_name(c"Ara")
                .api_version(vk::API_VERSION_1_3);
            let mut instance_info = vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_extension_names(&extensions)
                .enabled_layer_names(if cfg!(debug_assertions) {
                    &layer_names
                } else {
                    &[]
                })
                .push(&mut debug_info);
            if cfg!(debug_assertions) {
                instance_info = instance_info.push(&mut features);
            }
            let instance = entry.create_instance(&instance_info, None)?;
            let mut guard = super::super::guard::Initialization::new(instance.clone());
            let debug = ash::ext::debug_utils::Instance::load(&entry, &instance);
            let messenger = debug.create_debug_utils_messenger(&debug_info, None)?;
            guard.debug = Some((debug.clone(), messenger));
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
                .context("A dedicated compute-only family is required")?
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
            let rt_extensions = has_extension(ash::khr::acceleration_structure::NAME)
                && has_extension(ash::khr::ray_query::NAME)
                && has_extension(ash::khr::deferred_host_operations::NAME);
            let mut device_extensions = Vec::new();
            if !window_extensions.is_empty() {
                device_extensions.push(ash::khr::swapchain::NAME.as_ptr());
            }
            for &extension in &device_extensions {
                ensure!(
                    has_extension(CStr::from_ptr(extension)),
                    "Missing {:?}",
                    CStr::from_ptr(extension)
                );
            }
            let mut v11 = vk::PhysicalDeviceVulkan11Features::default();
            let mut v12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut v13 = vk::PhysicalDeviceVulkan13Features::default();
            let mut acceleration_features =
                vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
            let mut ray = vk::PhysicalDeviceRayQueryFeaturesKHR::default();
            let mut mesh_features = vk::PhysicalDeviceMeshShaderFeaturesEXT::default();
            let mut supported = vk::PhysicalDeviceFeatures2::default()
                .push(&mut v11)
                .push(&mut v12)
                .push(&mut v13)
                .push(&mut acceleration_features)
                .push(&mut mesh_features)
                .push(&mut ray);
            instance.get_physical_device_features2(physical, &mut supported);
            let core_supported = supported.features;
            let mesh_shading = has_extension(ash::ext::mesh_shader::NAME)
                && mesh_features.mesh_shader != 0
                && mesh_features.task_shader != 0;
            let buffer_int64_atomics =
                core_supported.shader_int64 != 0 && v12.shader_buffer_int64_atomics != 0;
            if mesh_shading {
                device_extensions.push(ash::ext::mesh_shader::NAME.as_ptr());
            }
            let ray_tracing = rt_extensions
                && acceleration_features.acceleration_structure != 0
                && ray.ray_query != 0
                && acceleration_features
                    .descriptor_binding_acceleration_structure_update_after_bind
                    != 0;
            if ray_tracing {
                device_extensions.extend([
                    ash::khr::acceleration_structure::NAME.as_ptr(),
                    ash::khr::ray_query::NAME.as_ptr(),
                    ash::khr::deferred_host_operations::NAME.as_ptr(),
                ]);
            }
            ensure!(
                v11.shader_draw_parameters != 0
                    && v13.shader_demote_to_helper_invocation != 0
                    && v12.timeline_semaphore != 0
                    && v12.buffer_device_address != 0
                    && v12.descriptor_indexing != 0
                    && v12.shader_sampled_image_array_non_uniform_indexing != 0
                    && v12.shader_storage_buffer_array_non_uniform_indexing != 0
                    && v12.descriptor_binding_sampled_image_update_after_bind != 0
                    && v12.descriptor_binding_storage_buffer_update_after_bind != 0
                    && v12.descriptor_binding_partially_bound != 0
                    && v12.descriptor_binding_storage_image_update_after_bind != 0
                    && v12.shader_storage_image_array_non_uniform_indexing != 0
                    && v13.dynamic_rendering != 0
                    && v13.synchronization2 != 0
                    && core_supported.vertex_pipeline_stores_and_atomics != 0
                    && core_supported.fragment_stores_and_atomics != 0
                    && core_supported.shader_storage_buffer_array_dynamic_indexing != 0
                    && core_supported.shader_sampled_image_array_dynamic_indexing != 0
                    && core_supported.shader_storage_image_array_dynamic_indexing != 0
                    && core_supported.shader_storage_image_extended_formats != 0
                    && core_supported.independent_blend != 0,
                "Missing required bindless/raster/sync features"
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
            let mut v11 =
                vk::PhysicalDeviceVulkan11Features::default().shader_draw_parameters(true);
            let mut v12 = vk::PhysicalDeviceVulkan12Features::default()
                .shader_buffer_int64_atomics(buffer_int64_atomics)
                .timeline_semaphore(true)
                .buffer_device_address(true)
                .descriptor_indexing(true)
                .shader_sampled_image_array_non_uniform_indexing(true)
                .shader_storage_buffer_array_non_uniform_indexing(true)
                .descriptor_binding_sampled_image_update_after_bind(true)
                .descriptor_binding_storage_buffer_update_after_bind(true)
                .descriptor_binding_partially_bound(true)
                .descriptor_binding_storage_image_update_after_bind(true)
                .shader_storage_image_array_non_uniform_indexing(true);
            let mut v13 = vk::PhysicalDeviceVulkan13Features::default()
                .shader_demote_to_helper_invocation(true)
                .dynamic_rendering(true)
                .synchronization2(true);
            let mut acceleration_features =
                vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
                    .acceleration_structure(ray_tracing)
                    .descriptor_binding_acceleration_structure_update_after_bind(ray_tracing);
            let mut ray = vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(ray_tracing);
            let core = vk::PhysicalDeviceFeatures::default()
                // Slang's fragment PrimitiveId interface declares Geometry.
                // Enabling the reported core feature also permits that interface
                // when the producer is a mesh stage rather than geometry stage.
                .geometry_shader(core_supported.geometry_shader != 0)
                .shader_int64(buffer_int64_atomics)
                .fragment_stores_and_atomics(true)
                .vertex_pipeline_stores_and_atomics(true)
                .shader_storage_buffer_array_dynamic_indexing(true)
                .shader_sampled_image_array_dynamic_indexing(true)
                .shader_storage_image_array_dynamic_indexing(true)
                .shader_storage_image_extended_formats(true)
                .independent_blend(true);
            let mut device_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&device_extensions)
                .enabled_features(&core)
                .push(&mut v11)
                .push(&mut v12)
                .push(&mut v13);
            if ray_tracing {
                device_info = device_info.push(&mut acceleration_features).push(&mut ray);
            }
            let mut mesh_features = vk::PhysicalDeviceMeshShaderFeaturesEXT::default()
                .mesh_shader(mesh_shading)
                .task_shader(mesh_shading);
            if mesh_shading {
                device_info = device_info.push(&mut mesh_features);
            }
            let device = instance.create_device(physical, &device_info, None)?;
            guard.device = Some(device.clone());
            let graphics = device.get_device_queue(graphics_family, 0);
            let compute = device.get_device_queue(compute_family, 0);
            let graphics_pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(graphics_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            guard.pools.push(graphics_pool);
            let compute_pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(compute_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            guard.pools.push(compute_pool);
            let acceleration = ash::khr::acceleration_structure::Device::load(&instance, &device);
            let memory = instance.get_physical_device_memory_properties(physical);
            let mut as_properties = vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
            let mut mesh_properties = vk::PhysicalDeviceMeshShaderPropertiesEXT::default();
            instance.get_physical_device_properties2(
                physical,
                &mut vk::PhysicalDeviceProperties2::default()
                    .push(&mut as_properties)
                    .push(&mut mesh_properties),
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
                vendor_id: properties.vendor_id,
                device_id: properties.device_id,
                ray_tracing,
                mesh_shading,
                mesh_dispatch: [
                    mesh_properties.max_mesh_work_group_count[0],
                    mesh_properties.max_mesh_work_group_count[1],
                    mesh_properties.max_mesh_work_group_count[2],
                    mesh_properties.max_mesh_work_group_total_count,
                ],
                task_dispatch: [
                    mesh_properties.max_task_work_group_count[0],
                    mesh_properties.max_task_work_group_count[1],
                    mesh_properties.max_task_work_group_count[2],
                    mesh_properties.max_task_work_group_total_count,
                ],
                geometry_shader: core_supported.geometry_shader != 0,
                buffer_int64_atomics,
                max_image_dimension: properties.limits.max_image_dimension2_d,
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
            guard.disarm();
            let mesh = ash::ext::mesh_shader::Device::load(&instance, &device);
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
                mesh,
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
}
