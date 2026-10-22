//! Optional Vulkan instance + device creation in the player (`--gpu-init`). It stands in for
//! the scene-independent startup a real player pays before its first frame, so the cold and
//! pooled boot numbers can show what a pre-spawned player hides.

use std::time::Instant;

use anyhow::{Result, anyhow};
use ash::vk;

use crate::protocol::GpuTimings;

/// A live instance and logical device, destroyed on drop.
pub struct Gpu {
    _entry: ash::Entry,
    instance: ash::Instance,
    device: ash::Device,
}

impl Gpu {
    /// Loads the Vulkan loader, creates a 1.3 instance (no explicit layers; the OS still injects
    /// registered implicit layers) and a device with one graphics queue on the first discrete
    /// GPU (or the first GPU).
    pub fn init() -> Result<(Self, GpuTimings)> {
        let t = Instant::now();
        // SAFETY: loads the system Vulkan loader; nothing else in this process uses Vulkan.
        let entry = unsafe { ash::Entry::load() }.map_err(|e| anyhow!("vulkan loader: {e}"))?;
        let loader_us = t.elapsed().as_micros() as u64;
        let t = Instant::now();
        let app = vk::ApplicationInfo::default()
            .application_name(c"ara-s5-player")
            .api_version(vk::API_VERSION_1_3);
        let info = vk::InstanceCreateInfo::default().application_info(&app);
        // SAFETY: `info` and `app` are valid for the duration of the call.
        let instance = unsafe { entry.create_instance(&info, None) }?;
        let instance_us = t.elapsed().as_micros() as u64;
        let t = Instant::now();
        // SAFETY: `instance` is live.
        let physical = unsafe { instance.enumerate_physical_devices() }?;
        let pick = physical
            .iter()
            .copied()
            .find(|&pd| {
                // SAFETY: `pd` was enumerated from the live instance.
                let props = unsafe { instance.get_physical_device_properties(pd) };
                props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU
            })
            .or_else(|| physical.first().copied())
            .ok_or_else(|| anyhow!("no Vulkan device"))?;
        // SAFETY: `pick` was enumerated from the live instance.
        let families = unsafe { instance.get_physical_device_queue_family_properties(pick) };
        let family = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or_else(|| anyhow!("no graphics queue"))? as u32;
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let device_info = vk::DeviceCreateInfo::default().queue_create_infos(&queues);
        // SAFETY: `device_info` references live arrays; `pick` belongs to `instance`.
        let device = unsafe { instance.create_device(pick, &device_info, None) }?;
        let timings = GpuTimings {
            loader_us,
            instance_us,
            device_us: t.elapsed().as_micros() as u64,
        };
        Ok((
            Self {
                _entry: entry,
                instance,
                device,
            },
            timings,
        ))
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: the device and instance are idle and destroyed once, device first.
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}
