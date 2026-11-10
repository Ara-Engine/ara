//! Prefer independent compute, while preserving a legal single-family fallback.
use anyhow::{Context, Result};
use ash::vk;
pub(super) fn select(families: &[vk::QueueFamilyProperties]) -> Result<(u32, u32)> {
    let graphics = families
        .iter()
        .position(|f| f.queue_count > 0 && f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .context("No graphics queue")?;
    let compute = families
        .iter()
        .enumerate()
        .filter(|(_, f)| f.queue_count > 0 && f.queue_flags.contains(vk::QueueFlags::COMPUTE))
        .min_by_key(|(i, f)| {
            (
                f.queue_flags.contains(vk::QueueFlags::GRAPHICS),
                *i != graphics,
            )
        })
        .map(|(i, _)| i)
        .context("No compute-capable queue")?;
    Ok((graphics as u32, compute as u32))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefers_dedicated_compute_and_falls_back_to_the_graphics_family() {
        let mixed = vk::QueueFamilyProperties::default()
            .queue_flags(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
            .queue_count(1);
        let compute = vk::QueueFamilyProperties::default()
            .queue_flags(vk::QueueFlags::COMPUTE)
            .queue_count(1);
        assert_eq!(select(&[mixed]).unwrap(), (0, 0));
        assert_eq!(select(&[mixed, compute]).unwrap(), (0, 1));
        assert_eq!(select(&[mixed, mixed]).unwrap(), (0, 0));
        let absent = vk::QueueFamilyProperties::default()
            .queue_flags(vk::QueueFlags::COMPUTE)
            .queue_count(0);
        assert_eq!(select(&[mixed, absent]).unwrap(), (0, 0));
        assert!(select(&[mixed.queue_flags(vk::QueueFlags::GRAPHICS)]).is_err());
    }
}
