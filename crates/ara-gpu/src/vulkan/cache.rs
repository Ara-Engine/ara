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
    compatible_identity(
        bytes,
        info.vendor_id,
        info.device_id,
        &info.pipeline_cache_uuid,
    )
}

pub(super) fn compatible_identity(bytes: &[u8], vendor: u32, device: u32, uuid: &[u8]) -> bool {
    if bytes.len() < 32 {
        return false;
    }
    let word =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    word(0) == 32
        && word(4) == 1
        && word(8) == vendor
        && word(12) == device
        && bytes[16..32] == *uuid
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_corrupt_and_foreign_headers() {
        let info = crate::vulkan::context::test_info();
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
