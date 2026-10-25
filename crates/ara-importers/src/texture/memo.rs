//! Identical texture work is shared across model parts and disposable workers.
//! Only supplied cache services do I/O; sources, quality and encoder ABI key it.
use super::*;
use ara_asset::{CachedImport, Digest};

#[cfg(test)]
pub(super) fn key(
    source: &[u8],
    settings: &TextureSettings,
    target: TargetPlatform,
    preset: CookPreset,
) -> Result<Digest> {
    key_with_backend(source, settings, target, preset, None)
}
pub(super) fn key_with_backend(
    source: &[u8],
    settings: &TextureSettings,
    target: TargetPlatform,
    preset: CookPreset,
    backend: Option<&str>,
) -> Result<Digest> {
    let preset = settings.preset.unwrap_or(preset);
    let mut settings = serde_json::to_value(settings)?;
    if let Some(settings) = settings.as_object_mut() {
        settings.remove("preset");
    }
    let value = serde_json::json!([
        "Ara intermediate texture v2",
        TextureImporter.version(),
        Digest::of(source),
        settings,
        target,
        preset,
        backend.unwrap_or("cpu-ispc050")
    ]);
    Ok(Digest::of(
        ara_json::to_canonical_string(&ara_json::sort_keys(&value)).as_bytes(),
    ))
}

pub(super) fn load(
    memo: Option<&dyn ara_asset::CacheBackend>,
    key: Digest,
) -> Result<Option<Vec<u8>>> {
    let Some(cache) = memo else {
        return Ok(None);
    };
    // Intermediate files need not survive a crash; final publication validates
    // and durably stores its own objects in the parent cache.
    let Ok(Some(CachedImport { mut output, .. })) = cache.load(key) else {
        return Ok(None);
    };
    if output.artifacts.len() != 1
        || output.artifacts[0].kind != ArtifactKind::Texture
        || output.artifacts[0].name != "texture.ktx2"
    {
        return Err(invalid("invalid intermediate texture cache entry"));
    }
    Ok(output.artifacts.pop().map(|artifact| artifact.bytes))
}

pub(super) fn store(
    memo: Option<&dyn ara_asset::CacheBackend>,
    key: Digest,
    bytes: &[u8],
) -> Result<()> {
    if let Some(cache) = memo {
        cache.store(
            key,
            &ImportOutput {
                artifacts: vec![Artifact {
                    name: "texture.ktx2".into(),
                    kind: ArtifactKind::Texture,
                    bytes: bytes.to_vec(),
                }],
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_asset::{Cancellation, FileCache, ImportMetrics};
    use image::ImageEncoder;

    #[test]
    fn shared_texture_work_keeps_bytes_and_skips_decode_and_encoding() {
        let image = image::RgbaImage::from_fn(17, 9, |x, y| {
            image::Rgba([x as u8 * 11, y as u8 * 23, 97, 255])
        });
        let mut source = Vec::new();
        image::codecs::png::PngEncoder::new(&mut source)
            .write_image(&image, 17, 9, image::ExtendedColorType::Rgba8)
            .expect("png");
        let root = tempfile::tempdir().expect("cache");
        let cache = FileCache::at(root.path()).expect("cache");
        let cancel = Cancellation::default();
        let mut ctx = ImportContext {
            metrics: None,
            cancellation: &cancel,
            progress: &mut |_| {},
        };
        let settings = TextureSettings::default();
        let target = TargetPlatform::Windows;
        let preset = CookPreset::Fast;
        let reference = cook_texture_with_preset(&source, &settings, target, preset, &mut ctx)
            .expect("uncached");
        let jobs = vec![(settings.clone(), source.clone()); 3];
        let first =
            cook_texture_batch(&jobs, target, preset, &mut ctx, Some(&cache)).expect("cold memo");
        assert_eq!(first, vec![reference.clone(); 3]);
        let metrics = ImportMetrics::default();
        ctx.metrics = Some(&metrics);
        assert_eq!(
            cook_texture_batch(&jobs, target, preset, &mut ctx, Some(&cache)).expect("warm memo"),
            first
        );
        assert_eq!(
            cook_texture_memo(&source, &settings, target, preset, &mut ctx, Some(&cache))
                .expect("same task"),
            reference
        );
        let times = metrics.snapshot();
        assert_eq!(times.texture_cooks, 0);
        assert_eq!(times.texture_cache_hits, 2);
        assert_eq!(times.decode_ms, 0.0);
        assert_eq!(times.mips_ms, 0.0);
        assert_eq!(times.compression_ms, 0.0);
        let balanced = key(&source, &settings, target, CookPreset::Balanced).expect("project key");
        let overridden = TextureSettings {
            preset: Some(CookPreset::Balanced),
            ..settings.clone()
        };
        assert_eq!(
            balanced,
            key(&source, &overridden, target, CookPreset::HighQuality)
                .expect("same effective preset")
        );
        let original = key(&source, &settings, target, preset).expect("key");
        assert_ne!(
            original,
            key(&source, &settings, target, CookPreset::HighQuality).expect("preset")
        );
        assert_ne!(
            original,
            key(&source, &settings, TargetPlatform::Linux, preset).expect("target")
        );
        assert_ne!(
            original,
            key(
                &source,
                &TextureSettings {
                    usage: TextureUsage::Linear,
                    ..settings
                },
                target,
                preset
            )
            .expect("usage")
        );
        source.push(0);
        assert_ne!(
            original,
            key(&source, &TextureSettings::default(), target, preset).expect("source")
        );
        cancel.cancel();
        assert!(cook_texture_batch(&jobs, target, preset, &mut ctx, Some(&cache)).is_err());
    }
}
