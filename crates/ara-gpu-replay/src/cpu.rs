//! Scalar reference; every bloom store rounds through f16 like its GPU target.
use half::f16;
/// An image in row-major order with top-left origin.
#[derive(Clone, Debug)]
pub struct Image {
    /// Width and height in pixels.
    pub size: [u32; 2],
    /// Linear RGBA texels.
    pub pixels: Vec<[f32; 4]>,
}
/// Fixed input state shared by the two native backends and scalar reference.
pub struct Fixture {
    /// Odd-size half-precision HDR input.
    pub hdr: Image,
    /// Half-precision 3D LUT, red varying fastest.
    pub lut: Vec<[f32; 4]>,
    /// Side length of the cube LUT.
    pub lut_size: u32,
    /// Exact Renderlab Settings ABI: five vec4 values.
    pub settings: [f32; 20],
}
fn half(v: [f32; 4]) -> [f32; 4] {
    v.map(|x| f16::from_f32(x).to_f32())
}
/// Deterministic 17x11 HDR gradient with an impulse and a nonidentity four-cube LUT.
pub fn fixture() -> Fixture {
    let size = [17, 11];
    let pixels = (0..size[0] * size[1])
        .map(|i| {
            let x = (i % size[0]) as f32;
            let y = (i / size[0]) as f32;
            half([
                0.03 + x * 0.13 + if i == 78 { 7. } else { 0. },
                0.08 + y * 0.12,
                0.12 + (x + y) * 0.035,
                1.,
            ])
        })
        .collect();
    let lut_size = 4;
    let lut = (0..64)
        .map(|i| {
            let r = (i % 4) as f32 / 3.;
            let g = (i / 4 % 4) as f32 / 3.;
            let b = (i / 16) as f32 / 3.;
            half([0.8 * r + 0.1 * g, 0.9 * g, 0.75 * b + 0.15 * r, 1.])
        })
        .collect();
    Fixture {
        hdr: Image { size, pixels },
        lut,
        lut_size,
        settings: [
            0.25, 0., 0., 0., 0., 0., 0., 0., 1.1, 0.85, 0.05, 0., 1.1, 0.95, 1., 1., 0., 0.5, 4.,
            0.,
        ],
    }
}
fn load(image: &Image, x: i32, y: i32) -> [f32; 4] {
    let x = x.clamp(0, image.size[0] as i32 - 1) as usize;
    let y = y.clamp(0, image.size[1] as i32 - 1) as usize;
    image.pixels[y * image.size[0] as usize + x]
}
fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] * (1. - t) + b[i] * t)
}
fn sample(image: &Image, uv: [f32; 2]) -> [f32; 4] {
    let x = uv[0] * image.size[0] as f32 - 0.5;
    let y = uv[1] * image.size[1] as f32 - 0.5;
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    mix(
        mix(load(image, ix, iy), load(image, ix + 1, iy), x - x.floor()),
        mix(
            load(image, ix, iy + 1),
            load(image, ix + 1, iy + 1),
            x - x.floor(),
        ),
        y - y.floor(),
    )
}
fn tent(image: &Image, uv: [f32; 2]) -> [f32; 4] {
    let mut out = [0.; 4];
    for y in -1i32..=1 {
        for x in -1i32..=1 {
            let w = ((2 - x.abs()) * (2 - y.abs())) as f32 / 16.;
            let p = sample(
                image,
                [
                    uv[0] + x as f32 / image.size[0] as f32,
                    uv[1] + y as f32 / image.size[1] as f32,
                ],
            );
            for c in 0..4 {
                out[c] += p[c] * w;
            }
        }
    }
    out
}
/// Scalar nine-tap tent followed by an RGBA16Float store.
pub fn downsample(input: &Image, size: [u32; 2]) -> Image {
    Image {
        size,
        pixels: (0..size[0] * size[1])
            .map(|i| {
                half(tent(
                    input,
                    [
                        (i % size[0]) as f32 / size[0] as f32 + 0.5 / size[0] as f32,
                        (i / size[0]) as f32 / size[1] as f32 + 0.5 / size[1] as f32,
                    ],
                ))
            })
            .collect(),
    }
}
/// Scalar Renderlab upsample with a 0.65 convex mix and RGBA16Float store.
pub fn upsample(input: &Image, second: &Image) -> Image {
    let size = input.size;
    Image {
        size,
        pixels: (0..size[0] * size[1])
            .map(|i| {
                let x = i % size[0];
                let y = i / size[0];
                half(mix(
                    load(input, x as i32, y as i32),
                    tent(
                        second,
                        [
                            (x as f32 + 0.5) / size[0] as f32,
                            (y as f32 + 0.5) / size[1] as f32,
                        ],
                    ),
                    0.65,
                ))
            })
            .collect(),
    }
}
fn lut_sample(f: &Fixture, rgb: [f32; 3]) -> [f32; 3] {
    let n = f.lut_size as usize;
    let at = rgb.map(|x| x.clamp(0., 1.) * (n - 1) as f32);
    let lo = at.map(|x| x.floor() as usize);
    let hi = lo.map(|x| (x + 1).min(n - 1));
    let t = at.map(|x| x - x.floor());
    let mut out = [0.; 3];
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let i = if x == 0 { lo[0] } else { hi[0] }
                    + n * (if y == 0 { lo[1] } else { hi[1] }
                        + n * if z == 0 { lo[2] } else { hi[2] });
                let w = if x == 0 { 1. - t[0] } else { t[0] }
                    * if y == 0 { 1. - t[1] } else { t[1] }
                    * if z == 0 { 1. - t[2] } else { t[2] };
                for (c, v) in out.iter_mut().enumerate() {
                    *v += f.lut[i][c] * w;
                }
            }
        }
    }
    out
}
/// Scalar tone/grade/LUT/vignette reference for fixtures with grain and aberration disabled.
pub fn tone(colour: &Image, bloom: &Image, f: &Fixture) -> Image {
    assert_eq!(f.settings[11], 0., "grain must be disabled in this fixture");
    assert_eq!(
        f.settings[16], 0.,
        "aberration must be disabled in this fixture"
    );
    let size = colour.size;
    let pixels = (0..size[0] * size[1])
        .map(|i| {
            let uv = [
                ((i % size[0]) as f32 + 0.5) / size[0] as f32,
                ((i / size[0]) as f32 + 0.5) / size[1] as f32,
            ];
            let hdr = mix(sample(colour, uv), sample(bloom, uv), f.settings[0]);
            let mut rgb = std::array::from_fn::<_, 3, _>(|c| {
                let x = (hdr[c] * f.settings[12 + c]).max(0.);
                let x = if f.settings[15] > 0.5 {
                    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0., 1.)
                } else {
                    x / (1. + x)
                };
                0.18 * (x / 0.18).max(0.).powf(f.settings[8])
            });
            let l = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
            rgb = rgb.map(|x| (l * (1. - f.settings[9]) + x * f.settings[9]).clamp(0., 1.));
            if f.settings[17] > 0. {
                let mapped = lut_sample(f, rgb);
                rgb = std::array::from_fn(|c| {
                    rgb[c] * (1. - f.settings[17]) + mapped[c] * f.settings[17]
                });
            }
            let r = uv.map(|x| x * 2. - 1.);
            let edge = ((r[0] * r[0] + r[1] * r[1] - 0.15) / (1.8 - 0.15)).clamp(0., 1.);
            let edge = edge * edge * (3. - 2. * edge);
            let rgb = rgb.map(|x| (x * (1. - f.settings[10] * edge)).clamp(0., 1.));
            [rgb[0], rgb[1], rgb[2], 1.]
        })
        .collect();
    Image { size, pixels }
}
