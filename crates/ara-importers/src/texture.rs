use crate::invalid;
use ara_asset::CookPreset;
use ara_asset::{
    Artifact, ArtifactKind, ImportContext, ImportInput, ImportOutput, Importer, Result,
    TargetPlatform,
};
use ara_asset_format::{TextureFormat, write_ktx2};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
mod memo;

/// Semantic texture usage selects transfer functions, mip filtering and BC format.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ara_schema::Schema,
)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase")]
pub enum TextureUsage {
    /// sRGB color (alpha remains linear), BC7 sRGB.
    #[default]
    Color,
    /// Linear RGBA data, BC7 UNORM.
    Linear,
    /// Tangent-space normals, normalized mips and BC5 XY.
    Normal,
    /// A single linear scalar channel, BC4.
    Mask,
    /// Nonnegative linear radiance, BC6H UFLOAT.
    Hdr,
}
/// Declarative texture import settings.
#[derive(Debug, Clone, Serialize, Deserialize, ara_schema::Schema)]
#[ara(module = "asset")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct TextureSettings {
    /// Optional quality override; absent inherits the source/project preset.
    /// Changing it does not re-encode other texture tasks.
    pub preset: Option<CookPreset>,
    /// How color/normal/mask values are interpreted.
    pub usage: TextureUsage,
    /// Generate the complete mip chain through 1×1.
    #[ara(default = true)]
    pub mipmaps: bool,
    /// Invert normal Y for DirectX-authored tangent-space inputs.
    pub flip_normal_y: bool,
    /// Source channel for BC4, R=0/G=1/B=2/A=3.
    #[ara(range(min = 0, max = 3))]
    pub mask_channel: u8,
    /// Maximum permitted source dimension (decode limits are also enforced).
    #[ara(default = 16384, range(min = 1, max = 16384))]
    pub max_dimension: u32,
}
impl Default for TextureSettings {
    fn default() -> Self {
        Self {
            preset: None,
            usage: TextureUsage::Color,
            mipmaps: true,
            flip_normal_y: false,
            mask_channel: 0,
            max_dimension: 16384,
        }
    }
}
/// PNG, JPEG, Radiance HDR and OpenEXR importer with Intel ISPC BCn encoding.
pub struct TextureImporter;
impl Importer for TextureImporter {
    fn id(&self) -> &str {
        "ara.texture"
    }
    fn version(&self) -> &str {
        "4-image025-ispc050-texture-presets-tiled-f32text"
    }
    fn register_settings(&self, db: &mut ara_schema::SchemaDb) {
        db.register::<TextureSettings>();
    }
    fn import(&self, input: &ImportInput, ctx: &mut ImportContext<'_>) -> Result<ImportOutput> {
        self.import_with_memo(input, ctx, None)
    }
    fn import_with_memo(
        &self,
        input: &ImportInput,
        ctx: &mut ImportContext<'_>,
        memo: Option<&dyn ara_asset::CacheBackend>,
    ) -> Result<ImportOutput> {
        let settings = if input.settings.is_null() {
            TextureSettings::default()
        } else {
            serde_json::from_value(input.settings.clone())?
        };
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "texture.ktx2".into(),
                kind: ArtifactKind::Texture,
                bytes: cook_texture_memo(
                    &input.source,
                    &settings,
                    input.target,
                    input.preset,
                    ctx,
                    memo,
                )?,
            }],
        })
    }
}
/// Cooks source pixels to GPU-native KTX2, with linear-light, alpha-aware mips.
/// Mobile profiles deliberately require a plugin encoder rather than shipping BCn.
pub fn cook_texture(
    source: &[u8],
    settings: &TextureSettings,
    target: TargetPlatform,
    ctx: &mut ImportContext<'_>,
) -> Result<Vec<u8>> {
    cook_texture_with_preset(source, settings, target, CookPreset::default(), ctx)
}
/// Cooks a specific cacheable quality preset, using the caller's current Rayon pool.
/// Registry execution supplies a bounded pool; direct callers can use `pool.install`.
pub fn cook_texture_with_preset(
    source: &[u8],
    settings: &TextureSettings,
    target: TargetPlatform,
    preset: CookPreset,
    ctx: &mut ImportContext<'_>,
) -> Result<Vec<u8>> {
    cook_texture_memo(source, settings, target, preset, ctx, None)
}
fn cook_texture_memo(
    source: &[u8],
    settings: &TextureSettings,
    target: TargetPlatform,
    preset: CookPreset,
    ctx: &mut ImportContext<'_>,
    memo: Option<&dyn ara_asset::CacheBackend>,
) -> Result<Vec<u8>> {
    ctx.cancellation.check()?;
    let preset = settings.preset.unwrap_or(preset);
    let key = memo
        .map(|_| memo::key(source, settings, target, preset))
        .transpose()?;
    if let Some(key) = key
        && let Some(bytes) = memo::load(memo, key)?
    {
        if let Some(metrics) = ctx.metrics {
            metrics.texture_task(true);
        }
        ctx.report(1.0, "texture ready (shared work)")?;
        return Ok(bytes);
    }
    let decoded = {
        ctx.report(0.0, "decode texture")?;
        let _timer = ctx.metrics.map(|m| m.start(ara_asset::ImportStage::Decode));
        decode_texture(source, settings, target, ctx.cancellation)?
    };
    let prepared = {
        ctx.report(0.15, "generate texture mips")?;
        let _timer = ctx.metrics.map(|m| m.start(ara_asset::ImportStage::Mips));
        generate_mips(decoded, ctx.cancellation)?
    };
    ctx.report(0.3, "encode texture tiles")?;
    let _timer = ctx
        .metrics
        .map(|m| m.start(ara_asset::ImportStage::Compression));
    let result = encode_texture(prepared, preset, ctx.cancellation)?;
    if let Some(metrics) = ctx.metrics {
        metrics.texture_task(false);
    }
    drop(_timer);
    if let Some(key) = key {
        memo::store(memo, key, &result)?;
    }
    ctx.report(1.0, "texture ready")?;
    Ok(result)
}
struct DecodedTexture {
    width: u32,
    height: u32,
    usage: TextureUsage,
    format: TextureFormat,
    pixels: Vec<[f32; 4]>,
    mipmaps: bool,
}
struct MippedTexture {
    width: u32,
    height: u32,
    usage: TextureUsage,
    format: TextureFormat,
    mips: Vec<(u32, u32, Vec<[f32; 4]>)>,
}
// Finish each phase across the entire batch before the next, so nested work
// stealing cannot starve decode/mips behind a slow BC kernel. Stage timings are
// elapsed wall time for the complete phase, including its parallel work.
pub(crate) fn cook_texture_batch(
    jobs: &[(TextureSettings, Vec<u8>)],
    target: TargetPlatform,
    preset: CookPreset,
    ctx: &mut ImportContext<'_>,
    memo: Option<&dyn ara_asset::CacheBackend>,
) -> Result<Vec<Vec<u8>>> {
    // Image indices/URIs can alias the same bytes. Deduplicate before decoding,
    // preserving original artifact order and all semantic texture settings.
    let mut unique = std::collections::BTreeMap::new();
    let mut keys = Vec::new();
    let mut remap = Vec::with_capacity(jobs.len());
    let mut sources = Vec::new();
    for (index, (settings, source)) in jobs.iter().enumerate() {
        let key = memo::key(source, settings, target, preset)?;
        let next = sources.len();
        let entry = *unique.entry(key).or_insert_with(|| {
            sources.push(index);
            keys.push(key);
            next
        });
        remap.push(entry);
    }
    let mut results = vec![None; sources.len()];
    let mut missing = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        ctx.cancellation.check()?;
        results[index] = memo::load(memo, *key)?;
        if results[index].is_some()
            && let Some(metrics) = ctx.metrics
        {
            metrics.texture_task(true);
        }
        if results[index].is_none() {
            missing.push(index);
        }
    }
    // A count-only cap retained gigabytes for eight 4K images per worker. Each
    // image already parallelizes mip rows and BC tiles, so large images do not
    // need to coexist to keep the bounded Rayon pool busy.
    let weights: Vec<_> = missing
        .iter()
        .map(|&i| texture_working_set(&jobs[sources[i]].1))
        .collect();
    let mut start = 0;
    while start < missing.len() {
        let end = start + batch_len(&weights[start..]);
        let batch = &missing[start..end];
        let batch_jobs: Vec<_> = batch.iter().map(|&i| &jobs[sources[i]]).collect();
        let cooked = cook_texture_batch_inner(
            &batch_jobs,
            target,
            preset,
            ctx,
            [
                0.3 + start as f32 / missing.len() as f32 * 0.6,
                (end - start) as f32 / missing.len() as f32 * 0.6,
            ],
        )?;
        for (&index, bytes) in batch.iter().zip(cooked) {
            if let Some(metrics) = ctx.metrics {
                metrics.texture_task(false);
            }
            memo::store(memo, keys[index], &bytes)?;
            results[index] = Some(bytes);
        }
        start = end;
    }
    ctx.report(0.9, "textures ready")?;
    let mut uses = vec![0usize; results.len()];
    for &index in &remap {
        uses[index] += 1;
    }
    Ok(remap
        .into_iter()
        .map(|i| {
            uses[i] -= 1;
            if uses[i] == 0 {
                results[i]
                    .take()
                    .expect("every unique texture was loaded or cooked")
            } else {
                results[i]
                    .as_ref()
                    .expect("shared texture still has a consumer")
                    .clone()
            }
        })
        .collect())
}

const BATCH_MEMORY_BYTES: usize = 512 * 1024 * 1024;
fn texture_working_set(source: &[u8]) -> usize {
    image::ImageReader::new(std::io::Cursor::new(source))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        // Float RGBA + full mip chain + native packing/decoder scratch. The
        // decoder still enforces its real limits; malformed headers run alone.
        .map_or(BATCH_MEMORY_BYTES, |(w, h)| {
            (w as usize).saturating_mul(h as usize).saturating_mul(32)
        })
}
fn batch_len(weights: &[usize]) -> usize {
    let mut used = 0usize;
    let mut count = 0;
    for &weight in weights.iter().take(8) {
        if count > 0 && used.saturating_add(weight) > BATCH_MEMORY_BYTES {
            break;
        }
        used = used.saturating_add(weight);
        count += 1;
    }
    count
}

fn cook_texture_batch_inner(
    jobs: &[&(TextureSettings, Vec<u8>)],
    target: TargetPlatform,
    preset: CookPreset,
    ctx: &mut ImportContext<'_>,
    progress: [f32; 2],
) -> Result<Vec<Vec<u8>>> {
    let cancellation = ctx.cancellation;
    ctx.report(progress[0], "decode textures in parallel")?;
    let decoded: Result<Vec<_>> = {
        let _timer = ctx.metrics.map(|m| m.start(ara_asset::ImportStage::Decode));
        jobs.par_iter()
            .map(|(settings, source)| decode_texture(source, settings, target, cancellation))
            .collect()
    };
    let decoded = decoded?;
    ctx.report(
        progress[0] + progress[1] / 3.0,
        "generate texture mips in parallel",
    )?;
    let prepared: Result<Vec<_>> = {
        let _timer = ctx.metrics.map(|m| m.start(ara_asset::ImportStage::Mips));
        decoded
            .into_par_iter()
            .map(|d| generate_mips(d, cancellation))
            .collect()
    };
    let prepared = prepared?;
    ctx.report(
        progress[0] + progress[1] * 2.0 / 3.0,
        "encode texture tiles in parallel",
    )?;
    let _timer = ctx
        .metrics
        .map(|m| m.start(ara_asset::ImportStage::Compression));
    prepared
        .into_par_iter()
        .with_max_len(1)
        .zip(jobs.par_iter())
        .map(|(p, (settings, _))| {
            encode_texture(p, settings.preset.unwrap_or(preset), cancellation)
        })
        .collect()
}
fn decode_texture(
    source: &[u8],
    settings: &TextureSettings,
    target: TargetPlatform,
    cancellation: &ara_asset::Cancellation,
) -> Result<DecodedTexture> {
    if matches!(target, TargetPlatform::Android | TargetPlatform::Ios) {
        return Err(invalid(
            "mobile texture profile requires an ASTC texture importer plugin",
        ));
    }
    if settings.mask_channel > 3 || settings.max_dimension == 0 || settings.max_dimension > 16384 {
        return Err(invalid("invalid texture settings"));
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(source))
        .with_guessed_format()
        .map_err(invalid)?;
    let hdr = matches!(
        reader.format(),
        Some(image::ImageFormat::Hdr | image::ImageFormat::OpenExr)
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(settings.max_dimension);
    limits.max_image_height = Some(settings.max_dimension);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(invalid)?.into_rgba32f();
    let (width, height) = image.dimensions();
    let usage = if hdr && settings.usage == TextureUsage::Color {
        TextureUsage::Hdr
    } else {
        settings.usage
    };
    let mut format = match usage {
        TextureUsage::Color => TextureFormat::Bc7Srgb,
        TextureUsage::Linear => TextureFormat::Bc7,
        TextureUsage::Normal => TextureFormat::Bc5,
        TextureUsage::Mask => TextureFormat::Bc4,
        TextureUsage::Hdr => TextureFormat::Bc6h,
    };
    let mut pixels: Vec<[f32; 4]> = bytemuck::allocation::try_cast_vec(image.into_raw())
        .unwrap_or_else(|(_, data)| data.as_chunks::<4>().0.to_vec());
    pixels.par_iter_mut().try_for_each(|p| -> Result<()> {
        cancellation.check()?;
        if p.iter().any(|x| !x.is_finite()) {
            return Err(invalid("nonfinite image sample"));
        }
        match usage {
            TextureUsage::Color => {
                for c in &mut p[..3] {
                    *c = srgb_to_linear(*c);
                }
            }
            TextureUsage::Normal => {
                let n = crate::mesh::normalize([
                    p[0] * 2.0 - 1.0,
                    (p[1] * 2.0 - 1.0) * if settings.flip_normal_y { -1.0 } else { 1.0 },
                    p[2] * 2.0 - 1.0,
                ]);
                p[..3].copy_from_slice(&n);
            }
            TextureUsage::Mask => p[0] = p[settings.mask_channel as usize],
            TextureUsage::Hdr => {}
            TextureUsage::Linear => {}
        }
        Ok(())
    })?;
    if usage == TextureUsage::Hdr {
        let requirement = pixels
            .par_iter()
            .map(|p| {
                if p.iter().any(|x| x.abs() > 65504.0) {
                    2
                } else if p[..3].iter().any(|x| *x < 0.0) || p[3] != 1.0 {
                    1
                } else {
                    0
                }
            })
            .max()
            .unwrap_or(0);
        format = match requirement {
            2 => TextureFormat::Rgba32Float,
            1 => TextureFormat::Rgba16Float,
            _ => format,
        };
    }
    Ok(DecodedTexture {
        width,
        height,
        usage,
        format,
        pixels,
        mipmaps: settings.mipmaps,
    })
}
fn generate_mips(
    decoded: DecodedTexture,
    cancellation: &ara_asset::Cancellation,
) -> Result<MippedTexture> {
    let DecodedTexture {
        width,
        height,
        usage,
        format,
        mut pixels,
        mipmaps,
    } = decoded;
    let (mut w, mut h) = (width, height);
    let mut mips = Vec::new();
    loop {
        cancellation.check()?;
        let next = if mipmaps && (w != 1 || h != 1) {
            Some(downsample(
                &pixels,
                w,
                h,
                (w / 2).max(1),
                (h / 2).max(1),
                usage,
            ))
        } else {
            None
        };
        mips.push((w, h, pixels));
        let Some(next) = next else { break };
        pixels = next;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    Ok(MippedTexture {
        width,
        height,
        usage,
        format,
        mips,
    })
}
fn encode_texture(
    prepared: MippedTexture,
    preset: CookPreset,
    cancellation: &ara_asset::Cancellation,
) -> Result<Vec<u8>> {
    let MippedTexture {
        width,
        height,
        usage,
        format,
        mips,
    } = prepared;
    let levels: Result<Vec<_>> = mips
        .into_par_iter()
        .with_max_len(1)
        .map(|(w, h, pixels)| {
            crate::texture_codec::compress(&pixels, w, h, usage, format, preset, cancellation)
        })
        .collect();
    let levels = levels?;
    Ok(write_ktx2(width, height, format, &levels)?)
}
pub(crate) fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn downsample(
    pixels: &[[f32; 4]],
    w: u32,
    h: u32,
    nw: u32,
    nh: u32,
    usage: TextureUsage,
) -> Vec<[f32; 4]> {
    (0..nw * nh)
        .into_par_iter()
        .map(|index| {
            let y = index / nw;
            let x = index % nw;
            let x0 = x as f32 * w as f32 / nw as f32;
            let x1 = (x + 1) as f32 * w as f32 / nw as f32;
            let y0 = y as f32 * h as f32 / nh as f32;
            let y1 = (y + 1) as f32 * h as f32 / nh as f32;
            let mut sum = [0.0; 4];
            let mut hdr_sum = [0.0f64; 4];
            let mut weight = 0.0;
            for sy in y0.floor() as u32..(y1.ceil() as u32).min(h) {
                for sx in x0.floor() as u32..(x1.ceil() as u32).min(w) {
                    let area = (x1.min((sx + 1) as f32) - x0.max(sx as f32))
                        * (y1.min((sy + 1) as f32) - y0.max(sy as f32));
                    let p = pixels[(sy * w + sx) as usize];
                    for i in 0..4 {
                        if usage == TextureUsage::Hdr {
                            hdr_sum[i] += p[i] as f64 * area as f64;
                            continue;
                        }
                        sum[i] += p[i]
                            * area
                            * if usage == TextureUsage::Color && i < 3 {
                                p[3]
                            } else {
                                1.0
                            };
                    }
                    weight += area;
                }
            }
            if usage == TextureUsage::Hdr {
                sum = hdr_sum.map(|v| (v / weight as f64) as f32);
            } else if usage == TextureUsage::Color {
                for i in 0..3 {
                    sum[i] = if sum[3] > 1e-20 { sum[i] / sum[3] } else { 0.0 };
                }
                sum[3] /= weight;
            } else {
                for c in &mut sum {
                    *c /= weight;
                }
            }
            if usage == TextureUsage::Normal {
                let n = crate::mesh::normalize([sum[0], sum[1], sum[2]]);
                sum[..3].copy_from_slice(&n);
            }
            sum
        })
        .collect()
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[test]
    fn large_images_bound_retained_mips_without_excluding_a_single_large_image() {
        assert_eq!(batch_len(&[4096 * 4096 * 32; 8]), 1);
        assert_eq!(batch_len(&[2048 * 2048 * 32; 8]), 4);
        assert_eq!(batch_len(&[1024 * 1024 * 32; 16]), 8);
        assert_eq!(batch_len(&[usize::MAX; 2]), 1);
        assert_eq!(batch_len(&[]), 0);
    }
}
