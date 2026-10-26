//! Verified capability hints; physical-device queries never create a VkDevice.
use ara_asset::{
    Artifact, ArtifactKind, BlockEncoder, CacheBackend, Cancellation, CookPreset, Digest,
    FileCache, ImportOutput, Result,
};
use ara_asset_format::TextureFormat;
use ash::vk::{self, TaggedStructure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Adapter {
    vendor: u32,
    device: u32,
    driver: u32,
    api: u32,
    uuid: [u8; 16],
    driver_uuid: [u8; 16],
    luid: [u8; 8],
    luid_valid: bool,
}
struct Instance(ash::Instance);
impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: This probe owns the instance and creates no devices, surfaces
        // or asynchronous work. All copied properties outlive no Vulkan handles.
        unsafe { self.0.destroy_instance(None) };
    }
}
fn adapters() -> Result<Vec<Adapter>> {
    // SAFETY: Load the platform Vulkan loader exactly as the normal device path;
    // its Entry stays live until the owned instance has been destroyed.
    let entry = unsafe { ash::Entry::load() }.map_err(crate::dispatch::unavailable)?;
    let application = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_3);
    let info = vk::InstanceCreateInfo::default().application_info(&application);
    // SAFETY: The create-info borrows initialized, live application metadata;
    // there are no extension chains or custom allocation callbacks.
    let instance = Instance(
        unsafe { entry.create_instance(&info, None) }.map_err(crate::dispatch::unavailable)?,
    );
    // SAFETY: The instance is live. Returned physical handles are used only by
    // synchronous property queries before instance destruction.
    let physical =
        unsafe { instance.0.enumerate_physical_devices() }.map_err(crate::dispatch::unavailable)?;
    let mut result = Vec::new();
    for device in physical {
        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push(&mut id);
        // SAFETY: device belongs to this instance; the complete initialized
        // output chain remains live for this synchronous Vulkan 1.1+ query.
        unsafe {
            instance
                .0
                .get_physical_device_properties2(device, &mut properties)
        };
        let p = properties.properties;
        result.push(Adapter {
            vendor: p.vendor_id,
            device: p.device_id,
            driver: p.driver_version,
            api: p.api_version,
            uuid: id.device_uuid,
            driver_uuid: id.driver_uuid,
            luid: id.device_luid,
            luid_valid: id.device_luid_valid == vk::TRUE,
        });
    }
    if result.is_empty() {
        return Err(crate::dispatch::unavailable("no physical Vulkan adapter"));
    }
    Ok(result)
}
fn key(backend: &str, hardware: &[Adapter]) -> Result<Digest> {
    // Preserve enumeration order: adapter selection can depend on it. The
    // semantic codec identity covers the immutable manifests and codec policy,
    // not unrelated worker/editor rebuilds or temporary resource paths.
    let value = serde_json::json!([
        "Ara encoder capability v1",
        backend,
        hardware,
        std::env::var_os("VK_DRIVER_FILES").map(|v| v.to_string_lossy().into_owned()),
        std::env::var_os("VK_ICD_FILENAMES").map(|v| v.to_string_lossy().into_owned())
    ]);
    Ok(Digest::of(
        ara_json::to_canonical_string(&ara_json::sort_keys(&value)).as_bytes(),
    ))
}

fn load_identity(cache: &FileCache, key: Digest, expected: &str) -> Result<Option<String>> {
    let Some(hit) = cache.load(key)? else {
        return Ok(None);
    };
    if hit.output.artifacts.len() != 1 || hit.output.artifacts[0].name != "identity.json" {
        return Ok(None);
    }
    let actual: String = match serde_json::from_slice(&hit.output.artifacts[0].bytes) {
        Ok(actual) => actual,
        Err(_) => return Ok(None),
    };
    Ok((actual == expected).then_some(actual))
}
fn context(memo: Option<&Path>) -> Result<(FileCache, Digest, String)> {
    let (cooked, cache) = crate::locations(memo)?;
    let identity = crate::Encoder::new(&cooked, &cache)?.backend().to_owned();
    let key = key(&identity, &adapters()?)?;
    let root = memo.map(Path::to_path_buf).unwrap_or(cache);
    Ok((FileCache::at(root.join("encoder-hints"))?, key, identity))
}
pub(crate) fn lookup(memo: Option<&Path>) -> Result<Option<String>> {
    if std::env::var("ARA_TEXTURE_ENCODER").is_ok_and(|v| v.eq_ignore_ascii_case("cpu")) {
        return Ok(None);
    }
    let (cache, key, expected) = context(memo)?;
    load_identity(&cache, key, &expected)
}

fn remember(memo: Option<&Path>, actual: &str) -> Result<()> {
    let (cache, key, expected) = context(memo)?;
    if actual != expected {
        return Err(crate::dispatch::unavailable(
            "codec cohort changed while negotiating",
        ));
    }
    let bytes =
        ara_json::to_canonical_string(&serde_json::Value::String(actual.into())).into_bytes();
    cache.store(
        key,
        &ImportOutput {
            artifacts: vec![Artifact {
                name: "identity.json".into(),
                kind: ArtifactKind::Custom,
                bytes,
            }],
        },
    )
}
pub(crate) struct Remembered {
    inner: Box<dyn BlockEncoder>,
    memo: Option<PathBuf>,
}
impl Remembered {
    pub(crate) fn new(inner: Box<dyn BlockEncoder>, memo: Option<&Path>) -> Self {
        Self {
            inner,
            memo: memo.map(Path::to_path_buf),
        }
    }
}
impl BlockEncoder for Remembered {
    fn backend(&self) -> &str {
        self.inner.backend()
    }
    fn service_endpoint(&self) -> Option<ara_asset::EncoderEndpoint> {
        self.inner.service_endpoint()
    }
    fn metrics(&self) -> BTreeMap<String, f64> {
        self.inner.metrics()
    }
    fn prepare(&self) -> Result<()> {
        self.inner.prepare()?;
        if let Err(error) = remember(self.memo.as_deref(), self.backend()) {
            ara_asset::record_import_trace(
                "texture.identity_store_failed",
                "encoder",
                serde_json::json!({"reason":error.to_string()}),
            );
        }
        Ok(())
    }
    fn encode(
        &self,
        format: TextureFormat,
        preset: CookPreset,
        blocks: &[u32],
        cancellation: &Cancellation,
    ) -> Result<Vec<u8>> {
        self.inner.encode(format, preset, blocks, cancellation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn adapter() -> Adapter {
        Adapter {
            vendor: 1,
            device: 2,
            driver: 3,
            api: 4,
            uuid: [5; 16],
            driver_uuid: [6; 16],
            luid: [7; 8],
            luid_valid: true,
        }
    }
    #[test]
    fn identity_memo_keys_cover_hardware_driver_and_codec() -> Result<()> {
        let original = key("codec-a", &[adapter()])?;
        assert_ne!(original, key("codec-b", &[adapter()])?);
        let mut changed = adapter();
        changed.driver += 1;
        assert_ne!(original, key("codec-a", &[changed])?);
        let mut changed = adapter();
        changed.luid[0] += 1;
        assert_ne!(original, key("codec-a", &[changed])?);
        let mut changed = adapter();
        changed.driver_uuid[0] += 1;
        assert_ne!(original, key("codec-a", &[changed])?);
        Ok(())
    }
    #[test]
    fn wrong_or_damaged_identity_is_a_miss() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cache = FileCache::at(root.path())?;
        let key = key("current-codec", &[adapter()])?;
        let bytes = ara_json::to_canonical_string(&serde_json::Value::String("old-codec".into()))
            .into_bytes();
        cache.store(
            key,
            &ImportOutput {
                artifacts: vec![Artifact {
                    name: "identity.json".into(),
                    kind: ArtifactKind::Custom,
                    bytes,
                }],
            },
        )?;
        assert!(load_identity(&cache, key, "current-codec")?.is_none());
        std::fs::write(cache.root().join("actions").join(key.to_string()), b"{torn")?;
        assert!(load_identity(&cache, key, "current-codec")?.is_none());
        Ok(())
    }
}
