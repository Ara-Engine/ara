//! Integer BC4/5/6H/7 encoding on a headless Vulkan device. Importers receive an
//! explicit encoding service; device creation and filesystem access stay here.

mod dispatch;
mod encoder;
mod split;
pub use encoder::Encoder;

/// Worker frontend with automatic headless GPU capability discovery and CPU fallback.
/// CPU requests never create a device; the host selects the backend before caching.
pub fn serve_worker(registry: &ara_asset::ImporterRegistry) -> ara_asset::Result<()> {
    ara_asset::serve_worker_with_encoder(registry, create_worker_encoder)
}
fn create_worker_encoder(
    memo: Option<&std::path::Path>,
) -> ara_asset::Result<Box<dyn ara_asset::BlockEncoder>> {
    if std::env::var("ARA_TEXTURE_ENCODER").is_ok_and(|value| value.eq_ignore_ascii_case("cpu")) {
        return Err(ara_asset::Error::EncoderUnavailable("CPU encoding explicitly selected".into()));
    }
    let cooked = ara_gpu::cooked::resource_dir(
        "ara-texture-gpu",
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("cooked"),
    );
    let cache = match memo {
        Some(root) => root.join("gpu-codec-pipelines"),
        None => std::env::var_os("ARA_CONFIG_DIR")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                ara_asset::Error::EncoderUnavailable(
                    "set a private ARA_CONFIG_DIR for a worker without a project cache".into(),
                )
            })?
            .join("gpu-codec-pipelines"),
    };
    Ok(Box::new(Encoder::new(&cooked, &cache)?))
}

#[cfg(test)]
mod bc45_reference;
#[cfg(test)]
mod bc45_tests;
#[cfg(test)]
mod bc6h_reference;
#[cfg(test)]
mod bc7_reference;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod readback_tests;
