use crate::invalid;
use ara_asset::{
    Artifact, ArtifactKind, ImportContext, ImportInput, ImportOutput, Importer, Result,
    TargetPlatform,
};
use ara_asset_format::{TextureFormat, write_ktx2};
use serde::{Deserialize, Serialize};

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
        "1-image025-ispc050"
    }
    fn register_settings(&self, db: &mut ara_schema::SchemaDb) {
        db.register::<TextureSettings>();
    }
    fn import(&self, input: &ImportInput, ctx: &mut ImportContext<'_>) -> Result<ImportOutput> {
        let settings = if input.settings.is_null() {
            TextureSettings::default()
        } else {
            serde_json::from_value(input.settings.clone())?
        };
        Ok(ImportOutput {
            artifacts: vec![Artifact {
                name: "texture.ktx2".into(),
                kind: ArtifactKind::Texture,
                bytes: cook_texture(&input.source, &settings, input.target, ctx)?,
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
    if matches!(target, TargetPlatform::Android | TargetPlatform::Ios) {
        return Err(invalid(
            "mobile texture profile requires an ASTC texture importer plugin",
        ));
    }
    if settings.mask_channel > 3 || settings.max_dimension == 0 || settings.max_dimension > 16384 {
        return Err(invalid("invalid texture settings"));
    }
    ctx.report(0.0, "decode texture")?;
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
    let mut pixels: Vec<[f32; 4]> = image.pixels().map(|p| p.0).collect();
    for p in &mut pixels {
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
            TextureUsage::Hdr => {
                if p.iter().any(|x| x.abs() > 65504.0) {
                    format = TextureFormat::Rgba32Float;
                } else if format != TextureFormat::Rgba32Float
                    && (p[..3].iter().any(|x| *x < 0.0) || p[3] != 1.0)
                {
                    format = TextureFormat::Rgba16Float;
                }
            }
            TextureUsage::Linear => {}
        }
    }
    let (mut w, mut h) = (width, height);
    let mut levels = Vec::new();
    let total = 32 - width.max(height).leading_zeros();
    loop {
        ctx.report(levels.len() as f32 / total as f32, "encode ISPC BCn mip")?;
        levels.push(compress(&pixels, w, h, usage, format));
        if !settings.mipmaps || (w == 1 && h == 1) {
            break;
        }
        let nw = (w / 2).max(1);
        let nh = (h / 2).max(1);
        pixels = downsample(&pixels, w, h, nw, nh, usage);
        w = nw;
        h = nh;
    }
    Ok(write_ktx2(width, height, format, &levels)?)
}
pub(crate) fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
fn quantize(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}
fn compress(
    pixels: &[[f32; 4]],
    width: u32,
    height: u32,
    usage: TextureUsage,
    format: TextureFormat,
) -> Vec<u8> {
    if matches!(
        format,
        TextureFormat::Rgba16Float | TextureFormat::Rgba32Float
    ) {
        let mut bytes = Vec::with_capacity(pixels.len() * format.block_bytes());
        for &v in pixels.iter().flatten() {
            if format == TextureFormat::Rgba16Float {
                bytes.extend(half::f16::from_f32(v).to_bits().to_le_bytes());
            } else {
                bytes.extend(v.to_le_bytes());
            }
        }
        return bytes;
    }
    let w = width.next_multiple_of(4);
    let h = height.next_multiple_of(4);
    let channels = match usage {
        TextureUsage::Mask => 1,
        TextureUsage::Normal => 2,
        _ => 4,
    };
    let sample_bytes = if usage == TextureUsage::Hdr { 2 } else { 1 };
    let mut data = Vec::with_capacity((w * h) as usize * channels * sample_bytes);
    for y in 0..h {
        for x in 0..w {
            let p = pixels[(y.min(height - 1) * width + x.min(width - 1)) as usize];
            if usage == TextureUsage::Hdr {
                for c in p {
                    data.extend(half::f16::from_f32(c).to_bits().to_le_bytes());
                }
            } else {
                for (i, c) in p.into_iter().take(channels).enumerate() {
                    data.push(quantize(match usage {
                        TextureUsage::Color if i < 3 => linear_to_srgb(c),
                        TextureUsage::Normal => c * 0.5 + 0.5,
                        _ => c,
                    }));
                }
            }
        }
    }
    match usage {
        TextureUsage::Mask => intel_tex_2::bc4::compress_blocks(&intel_tex_2::RSurface {
            data: &data,
            width: w,
            height: h,
            stride: w,
        }),
        TextureUsage::Normal => intel_tex_2::bc5::compress_blocks(&intel_tex_2::RgSurface {
            data: &data,
            width: w,
            height: h,
            stride: w * 2,
        }),
        TextureUsage::Hdr => intel_tex_2::bc6h::compress_blocks(
            &intel_tex_2::bc6h::very_slow_settings(),
            &intel_tex_2::RgbaSurface {
                data: &data,
                width: w,
                height: h,
                stride: w * 8,
            },
        ),
        _ => {
            let alpha = pixels.iter().any(|p| p[3] < 1.0);
            let settings = if alpha {
                intel_tex_2::bc7::alpha_slow_settings()
            } else {
                intel_tex_2::bc7::opaque_slow_settings()
            };
            intel_tex_2::bc7::compress_blocks(
                &settings,
                &intel_tex_2::RgbaSurface {
                    data: &data,
                    width: w,
                    height: h,
                    stride: w * 4,
                },
            )
        }
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
    let mut result = Vec::with_capacity((nw * nh) as usize);
    for y in 0..nh {
        for x in 0..nw {
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
            result.push(sum);
        }
    }
    result
}
