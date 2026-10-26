pub(crate) mod allocation;
mod buffer_memory;
mod create;
#[cfg(test)]
mod diagnostics_tests;
mod environment;
mod presentation;
mod resources;
pub use resources::image_barrier;
pub(crate) use resources::{BufferAllocation, ImageAllocation};

use anyhow::{Result, bail};
use ash::{Entry, vk};
use serde::Serialize;
use std::{
    ffi::{CStr, c_void},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Validation callback counters retained through Vulkan teardown.
#[derive(Default)]
pub struct Validation {
    /// Validation errors, including synchronization hazards.
    pub errors: AtomicUsize,
    /// Validation performance/general warnings.
    pub warnings: AtomicUsize,
    /// Structured callback messages; this mutex protects diagnostics only.
    pub messages: Mutex<Vec<Diagnostic>>,
}

/// Counts validation and performance messages. General messages come from the loader
/// and third-party implicit layers (overlays, capture hooks with stale manifests); they
/// describe the machine, not our API usage, so they are informational, recorded
/// once per message/process and never counted. Stable bus keys join child notices.
unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    kind: vk::DebugUtilsMessageTypeFlagsEXT,
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
        let counted = kind.intersects(
            vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        );
        if counted {
            let counter = if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
                &state.errors
            } else {
                &state.warnings
            };
            counter.fetch_add(1, Ordering::Relaxed);
        } else if !environment::first(&message) {
            return vk::FALSE;
        }
        let severity = if counted {
            severity
        } else {
            vk::DebugUtilsMessageSeverityFlagsEXT::INFO
        };
        let label = if counted { "VALIDATION" } else { "ENVIRONMENT" };
        eprintln!("{label} {severity:?}: {message}");
        // Validation can fire every frame: admission samples it before anything is built.
        let code = if counted {
            ara_diag::codes::GPU_VALIDATION
        } else {
            ara_diag::codes::GPU_ENVIRONMENT
        };
        if ara_diag::bus::admit(code) {
            let error = severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR);
            let d = if !counted {
                ara_diag::Diagnostic::info(code, message.clone())
            } else if error {
                ara_diag::Diagnostic::error(code, message.clone())
            } else {
                ara_diag::Diagnostic::warning(code, message.clone())
            };
            let metadata = if counted {
                ara_diag::bus::Meta::default()
            } else {
                environment::metadata(&message)
            };
            ara_diag::bus::push(d, metadata);
        }
        if let Ok(mut messages) = state.messages.lock() {
            messages.push(Diagnostic {
                code: if counted {
                    "ARA_GPU_VALIDATION"
                } else {
                    "ARA_GPU_ENVIRONMENT"
                }
                .into(),
                id: if (*data).p_message_id_name.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr((*data).p_message_id_name)
                        .to_string_lossy()
                        .into_owned()
                },
                severity: format!("{severity:?}"),
                message,
            });
        }
    }
    vk::FALSE
}

/// One validation-layer diagnostic suitable for a diagnostics bus.
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    /// `ARA_GPU_VALIDATION` (counted by `assert_clean`) or `ARA_GPU_ENVIRONMENT`
    /// (loader and third-party layer messages, recorded only).
    pub code: String,
    /// Layer message identifier.
    pub id: String,
    /// Error/warning for validation, informational for loader/overlay notices.
    pub severity: String,
    /// Full driver-provided explanation.
    pub message: String,
}

/// Driver identity and queue capabilities recorded with every measurement.
#[derive(Serialize)]
pub struct DeviceInfo {
    /// Native device name.
    pub name: String,
    /// PCI vendor identity used by the pipeline cache.
    pub vendor_id: u32,
    /// Device identity used by the pipeline cache.
    pub device_id: u32,
    /// Discrete adapter classification; shared-memory integrated/CPU devices use
    /// conservative raster auto-selection regardless of their reported heap size.
    pub discrete: bool,
    /// Sum of physical device-local heaps in MiB, recorded once at device creation.
    pub device_local_mib: u64,
    /// Optional RT-tier support; the raster baseline never requires it.
    pub ray_tracing: bool,
    /// Enabled BC4/BC5/BC6H/BC7 sampled textures, without CPU decompression.
    pub texture_compression_bc: bool,
    /// Allocated variable-count sampled-image slots at binding 5, capped at 65,536
    /// and by the device's update-after-bind set, stage and pool limits.
    pub sampled_image_capacity: u32,
    /// Enabled optional VK_EXT_mesh_shader mesh/task stages.
    pub mesh_shading: bool,
    /// Mesh indirect group limits: x, y, z, and the total group count.
    pub mesh_dispatch: [u32; 4],
    /// Task indirect group limits: x, y, z, and the total group count.
    pub task_dispatch: [u32; 4],
    /// Enabled core geometry capability (also used by Slang PrimitiveId inputs).
    pub geometry_shader: bool,
    /// VK_EXT_hdr_metadata enabled for native window presentation.
    pub hdr_metadata: bool,
    /// Enabled 64-bit shader integer operations and buffer atomics.
    pub buffer_int64_atomics: bool,
    /// Enabled optional Vulkan 1.3 integer dot products. Scalar shaders remain valid.
    pub shader_integer_dot_product: bool,
    /// Packed unsigned 4x8 integer dot products are accelerated in hardware.
    pub packed_integer_dot_accelerated: bool,
    /// Maximum supported 2D extent.
    pub max_image_dimension: u32,
    /// Supported Vulkan version.
    pub api_version: String,
    /// Native driver version, included in cache invalidation.
    pub driver_version: u32,
    /// Native graphics family index.
    pub graphics_family: u32,
    /// Native dedicated compute family index.
    pub compute_family: u32,
    /// Reported graphics queue capabilities.
    pub graphics_queue_flags: String,
    /// Reported compute queue capabilities.
    pub compute_queue_flags: String,
    /// Whether the optional EXT descriptor heap is advertised; v1 uses descriptor indexing.
    pub descriptor_heap_available: bool,
    /// Nanoseconds per timestamp tick.
    pub timestamp_period_ns: f32,
    /// Significant timestamp bits, graphics then compute.
    pub timestamp_valid_bits: [u32; 2],
    /// Enabled common device-clock extension, KHR preferred over EXT. Without
    /// this guarantee, timestamps from different queues cannot be combined.
    pub timestamp_clock: Option<&'static str>,
    /// Native driver pipeline-cache compatibility UUID.
    pub pipeline_cache_uuid: Vec<u8>,
    /// Enabled `VK_KHR_external_memory_win32` (enabled whenever available):
    /// shared surfaces can be exported and imported ([`super::external`]).
    pub external_memory: bool,
    /// Physical GPU and driver identity (`VkPhysicalDeviceIDProperties`).
    pub identity: crate::shared::DeviceIdentity,
}

/// Owns queue recording and allocations, retaining a shared native device lifetime.
pub struct Gpu {
    host_waits: super::waits::Counter,
    shared: Arc<NativeDevice>,
    presentation_only: bool,
    /// Native graphics queue owned by this recording context.
    pub graphics: vk::Queue,
    /// Scene queue; may alias graphics on devices with one graphics queue.
    pub scene_graphics: vk::Queue,
    /// Native dedicated compute queue (unused by presentation-only contexts).
    pub compute: vk::Queue,
    /// Graphics-family recording pool, exclusive to this context.
    pub graphics_pool: vk::CommandPool,
    /// Compute-family recording pool, exclusive to this context.
    pub compute_pool: vk::CommandPool,
    buffers: Vec<BufferAllocation>,
    buffer_memory: buffer_memory::BufferMemory,
    images: Vec<ImageAllocation>,
    structures: Vec<vk::AccelerationStructureKHR>,
}

/// Immutable native device lifetime shared by isolated startup recording owners.
/// Queue submission and allocation ownership remain in each [`Gpu`].
pub struct NativeDevice {
    /// Loader lifetime; must outlive all native function tables.
    pub _entry: Entry,
    /// Owned Vulkan instance for native integrations.
    pub instance: ash::Instance,
    /// Selected physical device.
    pub physical: vk::PhysicalDevice,
    /// Owned logical device; unsafe native calls must respect engine ownership.
    pub device: ash::Device,
    /// Optional RT extension dispatch table; inspect info.ray_tracing before use.
    pub acceleration: ash::khr::acceleration_structure::Device,
    /// Optional mesh-shader dispatch table; gated by info.mesh_shading.
    pub mesh: ash::ext::mesh_shader::Device,
    /// Immutable device, driver and capability provenance.
    pub info: DeviceInfo,
    /// Stable callback state, retained through instance teardown.
    pub validation: Box<Validation>,
    /// Queried acceleration-build scratch alignment.
    pub scratch_alignment: u64,
    memory: vk::PhysicalDeviceMemoryProperties,
    debug: ash::ext::debug_utils::Instance,
    messenger: vk::DebugUtilsMessengerEXT,
}

impl std::ops::Deref for Gpu {
    type Target = NativeDevice;
    fn deref(&self) -> &Self::Target {
        &self.shared
    }
}

impl Gpu {
    /// Cumulative host time in explicit GPU/presentation waits on this device.
    pub fn host_wait_stats(&self) -> super::HostWaitStats {
        self.host_waits.snapshot()
    }

    /// Native integrations wrap one leaf driver wait with this. Do not nest
    /// measured waits: frame accounting subtracts each interval exactly once.
    pub fn measure_host_wait<T>(&self, operation: impl FnOnce() -> T) -> T {
        self.host_waits.measure(operation)
    }
    /// Allocate one primary command buffer for the selected queue family.
    pub fn commands(&self, compute: bool) -> Result<vk::CommandBuffer> {
        crate::cpu_scope!("GPU.allocate_commands");
        anyhow::ensure!(
            !compute || !self.presentation_only,
            "Loading context is graphics-only"
        );
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
    /// # Safety
    /// `cmd` must belong to this device and be idle (all prior uses completed).
    pub unsafe fn begin(&self, cmd: vk::CommandBuffer) -> Result<()> {
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
        // SAFETY: This command buffer is fresh and has no submitted use.
        unsafe {
            self.begin(cmd)?;
        }
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
            self.measure_host_wait(|| self.device.queue_wait_idle(self.graphics))?;
            self.device.free_command_buffers(self.graphics_pool, &[cmd]);
        }
        Ok(())
    }

    /// Fail immediately if validation has reported any error or hazard.
    pub fn assert_clean(&self) -> Result<()> {
        let errors = self.validation.errors.load(Ordering::Relaxed);
        let warnings = self.validation.warnings.load(Ordering::Relaxed);
        if errors > 0 || warnings > 0 {
            bail!(
                "{errors} validation errors (including synchronization hazards), {warnings} warnings"
            );
        }
        Ok(())
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: Last owner; wait for all queues before destroying children, then parents.
        unsafe {
            // The loading wrapper resolves its own timeline, and raw presentation
            // completes before teardown. Waiting this shared queue here would
            // also wait for an unrelated game owner on single-queue adapters.
            if !self.presentation_only {
                let _ = self.wait_owned_queues();
            }
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
            for memory in self.buffer_memory.drain() {
                self.shared.device.free_memory(memory, None);
            }
            self.device.destroy_command_pool(self.compute_pool, None);
            self.device.destroy_command_pool(self.graphics_pool, None);
        }
    }
}

impl Drop for NativeDevice {
    fn drop(&mut self) {
        // SAFETY: Every recording/allocation owner and its workers have gone;
        // their queues completed before their pools and resources were released.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            self.debug
                .destroy_debug_utils_messenger(self.messenger, None);
            self.instance.destroy_instance(None);
        }
    }
}
