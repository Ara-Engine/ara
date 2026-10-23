//! Pipeline-cache seed validation before passing opaque bytes to a driver.
use super::DeviceInfo;
use std::{
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);

pub(super) fn persist(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(super) fn compatible(bytes: &[u8], info: &DeviceInfo) -> bool {
    if bytes.len() < 32 {
        return false;
    }
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    word(0) == 32
        && word(4) == 1
        && word(8) == info.vendor_id
        && word(12) == info.device_id
        && bytes[16..32] == info.pipeline_cache_uuid
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_corrupt_and_foreign_headers() {
        let info = DeviceInfo {
            mesh_shading: false,
            mesh_dispatch: [0; 4],
            task_dispatch: [0; 4],
            geometry_shader: false,
            hdr_metadata: false,
            buffer_int64_atomics: false,
            name: String::new(),
            vendor_id: 1,
            device_id: 2,
            ray_tracing: false,
            texture_compression_bc: false,
            sampled_image_capacity: 65_536,
            max_image_dimension: 16384,
            api_version: String::new(),
            driver_version: 3,
            graphics_family: 0,
            compute_family: 1,
            graphics_queue_flags: String::new(),
            compute_queue_flags: String::new(),
            descriptor_heap_available: false,
            timestamp_period_ns: 1.,
            timestamp_valid_bits: [64; 2],
            pipeline_cache_uuid: vec![7; 16],
        };
        let mut bytes = Vec::new();
        for word in [32u32, 1, 1, 2] {
            bytes.extend(word.to_le_bytes());
        }
        bytes.extend([7; 16]);
        assert!(compatible(&bytes, &info));
        bytes[8] = 3;
        assert!(!compatible(&bytes, &info));
        assert!(!compatible(&bytes[..10], &info));
    }
}
