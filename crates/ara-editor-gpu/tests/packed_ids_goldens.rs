//! The Scene overlays over a packed id image the test writes itself (a ground, two boxes
//! and an id-less pane: ids by construction), checked against committed goldens. No
//! renderer is involved, so it runs on either backend.
//!
//! The goldens were generated from the direct path (the removed raster renderer's R32F id
//! image) after a test proved it pixel-identical to the packed path (that test went with the
//! renderer); on Metal, `direct_and_packed_frames_agree_and_write_goldens` re-proves that on
//! this image and, with `ARA_UPDATE_GOLDENS=1`, rewrites the goldens from the direct frames.
//! Compared on another GPU, derivative-based antialiasing may differ by a level or two,
//! so the check is: no pixel off by more than 8, at most 0.5% off by more than 2.
//! Captures of every run go under `target/tmp/ara-editor-gpu-packed-ids-goldens`.
use ara_editor_gpu::{IdSource, NO_ID_HIT, NativeUi, Overlay, UiDevice, open_presentation};
use ara_ui::paint::{Box2, DisplayList, Draw};
use ara_ui::style::Color;
use ara_ui_render::{atlas::TextMode, gpu::Image, gpu::Renderer as UiRenderer};
use std::ops::Range;
use std::path::{Path, PathBuf};

const SIZE: [u32; 2] = [160, 120];
const PACKED: u32 = 0x7fff_0010;
const GROUPS: u32 = 0x7fff_0002;
const ICON: u32 = 0x7fff_0003;
/// The Direct source's image on Metal (an ara-gpu R32F texture).
#[cfg(target_os = "macos")]
const DIRECT: u32 = 0x7fff_0011;

/// A rectangle with the id its pixels hold and the colour the "scene" paints there.
type Shape = (Range<u32>, Range<u32>, u32, [u8; 4]);
/// Back to front. Channels 1 and 2 are the boxes, 3 the ground (terrain); the pane has no
/// selectable id.
const SHAPES: [Shape; 4] = [
    (0..160, 80..120, 3, [96, 78, 60, 255]),
    (20..60, 40..80, 1, [190, 90, 70, 255]),
    (90..140, 30..80, 2, [70, 120, 190, 255]),
    (66..84, 20..70, NO_ID_HIT, [200, 220, 230, 255]),
];
const SKY: [u8; 4] = [120, 140, 160, 255];

fn viewport() -> u32 {
    ara_ui::source::stable_id("editor-viewport") as u32 & 0x7fff_ffff
}

/// The id at a pixel: the last shape covering it, 0 for the sky.
fn id_at(x: u32, y: u32) -> u32 {
    SHAPES
        .iter()
        .rev()
        .find(|(xs, ys, ..)| xs.contains(&x) && ys.contains(&y))
        .map_or(0, |shape| shape.2)
}

fn each_pixel() -> impl Iterator<Item = (u32, u32)> {
    (0..SIZE[1]).flat_map(|y| (0..SIZE[0]).map(move |x| (x, y)))
}

/// The packed id image: the 32-bit id little-endian across R, G, B, A.
fn packed_ids() -> Vec<u8> {
    each_pixel()
        .flat_map(|(x, y)| id_at(x, y).to_le_bytes())
        .collect()
}

/// What the scene looks like under the overlays.
fn colours() -> Vec<u8> {
    each_pixel()
        .flat_map(|(x, y)| {
            SHAPES
                .iter()
                .rev()
                .find(|(xs, ys, ..)| xs.contains(&x) && ys.contains(&y))
                .map_or(SKY, |shape| shape.3)
        })
        .collect()
}

/// The group table the editor builds: texel = channel; red + green the hover group, blue
/// the outline colour group (low 7 bits) and ground (bit 7).
fn group_table() -> Vec<u8> {
    let mut table = vec![0u8; 4096 * 4];
    for (channel, texel) in [
        (1usize, [1, 0, 1, 255]),
        (2, [2, 0, 0, 255]),
        (3, [3, 0, 0x80, 255]),
    ] {
        table[channel * 4..channel * 4 + 4].copy_from_slice(&texel);
    }
    table
}

const ORANGE: [f32; 4] = [1., 0.5, 0., 1.];
const CYAN: [f32; 4] = [0.3, 0.8, 1., 1.];

fn grid() -> Overlay {
    Overlay::Grid {
        // Looking down 57 degrees from 3 m: every ray reaches the ground plane.
        camera: [0., 3., 0., 0.],
        parameters: [1., 25f32.to_radians().tan(), 1., 60.],
        color: [1., 1., 1., 0.6],
        axes: [[1., 0.2, 0.2, 0.7], [0.2, 0.3, 1., 0.7]],
    }
}

fn outline() -> Overlay {
    Overlay::Outline {
        index: 1,
        color: ORANGE,
    }
}

fn hover() -> Overlay {
    Overlay::Hover {
        uv: [115. / SIZE[0] as f32, 50. / SIZE[1] as f32],
        color: CYAN,
    }
}

/// A gizmo on box A and an icon over it.
fn gizmo() -> Vec<Overlay> {
    let at = [40., 60.];
    let mut overlays = Vec::new();
    for (to, color) in [
        ([at[0] + 30., at[1]], [0.9, 0.2, 0.2, 1.]),
        ([at[0], at[1] - 30.], [0.2, 0.8, 0.2, 1.]),
    ] {
        overlays.push(Overlay::Line {
            from: at,
            to,
            width: 1.5,
            color,
        });
        overlays.push(Overlay::Quad {
            corners: [
                [to[0] - 3., to[1] - 3.],
                [to[0] + 3., to[1] - 3.],
                [to[0] + 3., to[1] + 3.],
                [to[0] - 3., to[1] + 3.],
            ],
            color,
        });
    }
    overlays.push(Overlay::Icon {
        rect: [44., 24., 12., 12.],
        image: ICON,
        color: [1., 0.9, 0.3, 1.],
    });
    overlays
}

/// The frames the goldens hold, by name.
fn frames() -> Vec<(&'static str, Vec<Overlay>)> {
    let mut all = vec![grid(), outline(), hover()];
    all.extend(gizmo());
    vec![
        ("blank", Vec::new()),
        ("grid", vec![grid()]),
        ("outline", vec![outline()]),
        ("hover", vec![hover()]),
        ("all", all),
    ]
}

struct Rig {
    ui: NativeUi,
    paint: UiRenderer,
    output: PathBuf,
    /// Dropped after the adapter that was made on it.
    _device: UiDevice,
}

impl Rig {
    fn new() -> ara_ui::Result<Self> {
        let output =
            Path::new(env!("CARGO_TARGET_TMPDIR")).join("ara-editor-gpu-packed-ids-goldens");
        std::fs::create_dir_all(&output)?;
        let device = open_presentation(&[])?;
        // SAFETY: `device` is the rig's last field, so it outlives the adapter; the test
        // thread owns every queue.
        let ui = unsafe { NativeUi::new(&device)? };
        let mut paint = UiRenderer::default();
        let image = |rgba| Image {
            width: SIZE[0],
            height: SIZE[1],
            rgba,
        };
        paint.image(viewport(), image(colours()))?;
        paint.image(PACKED, image(packed_ids()))?;
        paint.image(
            GROUPS,
            Image {
                width: 4096,
                height: 1,
                rgba: group_table(),
            },
        )?;
        paint.image(
            ICON,
            Image {
                width: 8,
                height: 8,
                rgba: [255, 255, 255, 220].repeat(64),
            },
        )?;
        Ok(Self {
            ui,
            paint,
            output,
            _device: device,
        })
    }

    /// The frame with `overlays` over the scene, reading ids from `ids`.
    fn frame(
        &mut self,
        name: &str,
        ids: IdSource,
        overlays: Vec<Overlay>,
    ) -> ara_ui::Result<image::RgbaImage> {
        let rect = Box2 {
            x: 0.,
            y: 0.,
            w: SIZE[0] as f32,
            h: SIZE[1] as f32,
        };
        self.ui.set_dpi(1.)?;
        self.ui.outline_groups(Some(GROUPS));
        self.ui.overlays(rect, ids, overlays);
        let list = DisplayList {
            commands: vec![Draw::Image {
                node: 1,
                rect,
                image: viewport(),
                tint: Color(255, 255, 255, 255),
            }],
            glyphs: Vec::new(),
        };
        self.paint
            .render(&mut self.ui, &list, &[], SIZE, TextMode::Raster)?;
        let frame = self.ui.pixels()?;
        frame.save(self.output.join(format!("{name}.png")))?;
        Ok(frame)
    }
}

fn golden(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/packed-ids")
        .join(format!("{name}.png"))
}

fn updating() -> bool {
    std::env::var_os("ARA_UPDATE_GOLDENS").is_some()
}

/// Pixels off by more than `level` in any channel.
fn off_by_more_than(a: &image::RgbaImage, b: &image::RgbaImage, level: u8) -> (usize, u8) {
    let mut count = 0;
    let mut worst = 0;
    for (a, b) in a.pixels().zip(b.pixels()) {
        let delta =
            a.0.iter()
                .zip(b.0)
                .map(|(a, b)| a.abs_diff(b))
                .max()
                .unwrap_or(0);
        worst = worst.max(delta);
        count += usize::from(delta > level);
    }
    (count, worst)
}

#[test]
fn packed_overlays_match_their_goldens() -> ara_ui::Result<()> {
    if updating() {
        return Ok(());
    }
    let mut rig = Rig::new()?;
    let blank = rig.frame("blank", IdSource::Packed(PACKED), Vec::new())?;
    for (name, overlays) in frames() {
        let frame = rig.frame(name, IdSource::Packed(PACKED), overlays)?;
        let expected = image::open(golden(name))
            .map_err(|e| format!("golden {name}: {e}"))?
            .to_rgba8();
        assert_eq!(expected.dimensions(), frame.dimensions(), "{name}: size");
        let (slight, worst) = off_by_more_than(&frame, &expected, 2);
        assert!(
            worst <= 8 && slight <= (SIZE[0] * SIZE[1]) as usize / 200,
            "{name}: {slight} pixels off by more than 2, worst {worst} (captures in {})",
            rig.output.display()
        );
        // A golden that is the blank frame would prove nothing.
        if name != "blank" {
            assert!(off_by_more_than(&frame, &blank, 0).0 > 0, "{name} draws");
        }
    }
    Ok(())
}

/// Metal: the same ids as an ara-render-style R32F image (the direct path) compose the same
/// frames as the packed image, pixel for pixel; `ARA_UPDATE_GOLDENS=1` rewrites the goldens
/// from the direct frames.
#[cfg(target_os = "macos")]
#[test]
fn direct_and_packed_frames_agree_and_write_goldens() -> ara_ui::Result<()> {
    use ara_gpu::commands::Command;
    use ara_gpu::metal::MetalDevice;
    use ara_gpu::{Access, Barrier, Queue, Resource, vk};

    let mut rig = Rig::new()?;
    // The id image as ara-render holds it: R32_SFLOAT, the id as a float.
    let mut gpu = MetalDevice::new_raster(&[], rig.output.join("cache"))?;
    let image = gpu.create_image(
        SIZE[0],
        SIZE[1],
        vk::Format::R32_SFLOAT,
        vk::ImageUsageFlags::TRANSFER_DST
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::SAMPLED,
    )?;
    let floats: Vec<u8> = each_pixel()
        .flat_map(|(x, y)| (id_at(x, y) as f32).to_le_bytes())
        .collect();
    let staging = gpu.create_buffer(
        floats.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        true,
    )?;
    gpu.upload(staging, &floats)?;
    let barrier = |before, after| {
        Command::Barrier(Barrier {
            resource: Resource::Image(image),
            before,
            after,
            handoff: None,
        })
    };
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(barrier(Access::Undefined, Access::CopyDestination));
    rec.push(Command::UploadImage {
        source: staging,
        destination: image,
    });
    rec.push(barrier(Access::CopyDestination, Access::CopySource));
    let point = gpu.submit(rec.finish(), &[])?;
    gpu.resolve(point)?;
    // SAFETY: the image is complete, in CopySource, and `gpu` outlives the adapter's use.
    unsafe {
        rig.ui
            .external_image(DIRECT, gpu.native_image(image)?, image)?;
    }
    rig.paint.external_image(DIRECT)?;

    for (name, overlays) in frames() {
        let direct = rig.frame(
            &format!("{name}-direct"),
            IdSource::Direct(DIRECT),
            overlays_again(name),
        )?;
        let packed = rig.frame(
            &format!("{name}-packed"),
            IdSource::Packed(PACKED),
            overlays,
        )?;
        assert_eq!(
            off_by_more_than(&direct, &packed, 0).0,
            0,
            "{name}: the packed path composes the direct frame"
        );
        if updating() {
            std::fs::create_dir_all(golden(name).parent().expect("golden directory"))?;
            direct.save(golden(name))?;
        }
    }
    Ok(())
}

/// The overlays of frame `name`, built again (overlays are not `Clone`).
#[cfg(target_os = "macos")]
fn overlays_again(name: &str) -> Vec<Overlay> {
    frames()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, overlays)| overlays)
        .expect("a known frame")
}
