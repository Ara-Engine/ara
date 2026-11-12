//! Explicit publication measurement; ordinary tests never enforce timing limits.
use super::*;
use ara_asset_format::{TextureFormat, write_ktx2};
use std::time::Instant;

#[test]
#[ignore = "paired texture publication benchmark; run under ara-dev gpu"]
fn native_publication_preallocation_measurement() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-texture-stream/native-publication-cost");
    let mut samples = Vec::new();
    for repeat in 0..2 {
        for asynchronous in [false, true] {
            let mut gpu = GpuDevice::new_raster(&[], root.join("gpu"))?;
            let mut set = NativeResidency::new(NativeConfig {
                resident_bytes: 128 << 20,
                upload_bytes: 4 << 20,
            })?;
            for index in 0..32u8 {
                let levels: Vec<_> = (0..=10)
                    .map(|level| {
                        let mut bytes =
                            vec![0; (((1024u32 >> level).max(1)).div_ceil(4).pow(2) * 16) as usize];
                        for block in bytes.as_chunks_mut::<16>().0 {
                            block[0] = 0x40;
                            block[1] = index;
                        }
                        bytes
                    })
                    .collect();
                let bytes = write_ktx2(1024, 1024, TextureFormat::Bc7Srgb, &levels)?;
                use sha2::Digest;
                let key = sha2::Sha256::digest(&bytes).into();
                set.register(key, Arc::new(NativeSource::from_cooked(bytes.into())?))?;
            }
            let begin = Instant::now();
            let mut preparation_owner_ms = 0.;
            if asynchronous {
                loop {
                    let start = Instant::now();
                    let ready = set.preallocate(&mut gpu, None)?;
                    preparation_owner_ms += start.elapsed().as_secs_f64() * 1000.;
                    if ready {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            let preparation_wall_ms = begin.elapsed().as_secs_f64() * 1000.;
            gpu.begin_deferred_completion_on_current_queue()?;
            let start = Instant::now();
            let result = set.initialize(&mut gpu);
            let publish_owner_ms = start.elapsed().as_secs_f64() * 1000.;
            let mut completion = gpu.end_deferred_completion()?;
            gpu.wait_completion(&mut completion)?;
            let stats = result?;
            samples.push(serde_json::json!({
                "repeat": repeat, "asynchronous": asynchronous,
                "maps": stats.textures, "payload_bytes": stats.resident_bytes,
                "publish_owner_ms": publish_owner_ms,
                "preparation_owner_ms": preparation_owner_ms,
                "preparation_wall_ms": preparation_wall_ms,
                "through_upload_ms": begin.elapsed().as_secs_f64() * 1000.,
            }));
            set.destroy(&mut gpu)?;
            gpu.assert_clean()?;
        }
    }
    std::fs::create_dir_all(&root)?;
    let text = serde_json::to_string_pretty(&serde_json::json!({"samples": samples}))?;
    std::fs::write(root.join("paired.json"), &text)?;
    println!("{text}");
    Ok(())
}
