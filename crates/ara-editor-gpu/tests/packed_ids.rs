//! The Scene overlays read object ids from a packed RGBA8 image (`IdSource::Packed`, the
//! form Renderlab delivers) exactly: a 32-bit id little-endian across the channels, an
//! exact texel fetch, and `NO_ID_HIT` for a hit that is there but not selectable. The
//! image here is a UI texture with chosen bytes, so the test runs on either backend with
//! no renderer. Captures go under `target/tmp/ara-editor-gpu-packed-ids`.
use ara_editor_gpu::{IdSource, NO_ID_HIT, NativeUi, Overlay, UiDevice, open_presentation};
use ara_ui::paint::{Box2, DisplayList, Draw};
use ara_ui::style::Color;
use ara_ui_render::{atlas::TextMode, gpu::Image, gpu::Renderer as UiRenderer};

const SIZE: [u32; 2] = [96, 64];
/// A multi-byte id past 16 bits that still fits a float exactly (the outline's wanted id).
const BIG_ID: u32 = 0x00AB_CDEF;
const PACKED: u32 = 0x7fff_0010;

/// The Scene viewport's UI image handle, which the adapter decorates with overlays.
fn viewport() -> u32 {
    ara_ui::source::stable_id("editor-viewport") as u32 & 0x7fff_ffff
}

/// A pixel rectangle: x range and y range.
type Region = (std::ops::Range<u32>, std::ops::Range<u32>);
const BIG: Region = (8..40, 8..40);
const UNSELECTABLE: Region = (52..88, 8..40);

fn inside(region: &Region, x: u32, y: u32) -> bool {
    region.0.contains(&x) && region.1.contains(&y)
}

/// Pixels of `a` and `b` that differ inside `region` (the whole frame when `None`).
fn differing(a: &image::RgbaImage, b: &image::RgbaImage, region: Option<&Region>) -> usize {
    a.enumerate_pixels()
        .filter(|(x, y, p)| region.is_none_or(|r| inside(r, *x, *y)) && *p != b.get_pixel(*x, *y))
        .count()
}

struct Rig {
    ui: NativeUi,
    paint: UiRenderer,
    output: std::path::PathBuf,
    /// Dropped after the adapter that was made on it.
    _device: UiDevice,
}

impl Rig {
    /// A flat viewport image and the packed id image: `BIG_ID` in `BIG`, `NO_ID_HIT` in
    /// `UNSELECTABLE`, nothing elsewhere.
    fn new() -> ara_ui::Result<Self> {
        let output =
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("ara-editor-gpu-packed-ids");
        std::fs::create_dir_all(&output)?;
        let device = open_presentation(&[])?;
        // SAFETY: `device` is the rig's last field, so it outlives the adapter; the test
        // thread owns every queue.
        let ui = unsafe { NativeUi::new(&device)? };
        let mut paint = UiRenderer::default();
        paint.image(
            viewport(),
            Image {
                width: SIZE[0],
                height: SIZE[1],
                rgba: [60, 60, 70, 255].repeat((SIZE[0] * SIZE[1]) as usize),
            },
        )?;
        let mut ids = Vec::new();
        for y in 0..SIZE[1] {
            for x in 0..SIZE[0] {
                let id = if inside(&BIG, x, y) {
                    BIG_ID
                } else if inside(&UNSELECTABLE, x, y) {
                    NO_ID_HIT
                } else {
                    0
                };
                ids.extend_from_slice(&id.to_le_bytes());
            }
        }
        paint.image(
            PACKED,
            Image {
                width: SIZE[0],
                height: SIZE[1],
                rgba: ids,
            },
        )?;
        Ok(Self {
            ui,
            paint,
            output,
            _device: device,
        })
    }

    /// The frame with `overlays` over the viewport, reading ids from the packed image.
    fn frame(&mut self, name: &str, overlays: Vec<Overlay>) -> ara_ui::Result<image::RgbaImage> {
        let rect = Box2 {
            x: 0.,
            y: 0.,
            w: SIZE[0] as f32,
            h: SIZE[1] as f32,
        };
        self.ui.set_dpi(1.)?;
        self.ui.overlays(rect, IdSource::Packed(PACKED), overlays);
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

fn outline(index: u32) -> Overlay {
    Overlay::Outline {
        index,
        color: [1., 0.5, 0., 1.],
    }
}

fn hover(x: u32, y: u32) -> Overlay {
    Overlay::Hover {
        uv: [
            (x as f32 + 0.5) / SIZE[0] as f32,
            (y as f32 + 0.5) / SIZE[1] as f32,
        ],
        color: [1., 0.5, 0., 1.],
    }
}

fn grid() -> Overlay {
    Overlay::Grid {
        camera: [0., 3., 0., 0.],
        // Looking down at 57 degrees with a 25 degree half field of view: every ray
        // reaches the ground.
        parameters: [1., 25f32.to_radians().tan(), 1., 60.],
        color: [1., 1., 1., 1.],
        axes: [[1., 0., 0., 1.], [0., 0., 1., 1.]],
    }
}

#[test]
fn packed_ids_outline_hover_and_occlude_exactly() -> ara_ui::Result<()> {
    let mut rig = Rig::new()?;
    let blank = rig.frame("blank", Vec::new())?;

    // A multi-byte id outlines its own region's edge: the decode is exact in every
    // channel, or the wanted id would match no texel.
    let outlined = rig.frame("outline-big-id", vec![outline(BIG_ID)])?;
    assert!(
        differing(&outlined, &blank, Some(&BIG)) > 0,
        "the outline of {BIG_ID:#x} reaches the frame"
    );
    assert_eq!(
        differing(&outlined, &blank, Some(&(20..28, 20..28))),
        0,
        "the middle of the shape stays clear"
    );
    // The id-less hit is a shape with no outline, and no edge of the outlined shape's
    // neighbours either.
    assert_eq!(
        differing(&outlined, &blank, Some(&(44..SIZE[0], 0..SIZE[1]))),
        0,
        "NO_ID_HIT is never outlined"
    );
    assert_eq!(
        differing(
            &rig.frame("outline-no-hit", vec![outline(0xFFFFFF)])?,
            &blank,
            None
        ),
        0,
        "no texel holds 0xFFFFFF, so nothing outlines"
    );

    // Hover reads the id under the pointer through the same decode.
    let hovered = rig.frame("hover-big-id", vec![hover(24, 24)])?;
    assert_eq!(
        differing(&hovered, &outlined, None),
        0,
        "hovering a shape outlines it as its outline does"
    );
    let over_unselectable = rig.frame("hover-no-hit", vec![hover(70, 24)])?;
    assert_eq!(
        differing(&over_unselectable, &blank, None),
        0,
        "hovering a hit without an id outlines nothing"
    );
    let over_nothing = rig.frame("hover-nothing", vec![hover(4, 60)])?;
    assert_eq!(
        differing(&over_nothing, &blank, None),
        0,
        "hovering the background outlines nothing"
    );

    // Anything drawn hides the grid, selectable or not.
    let gridded = rig.frame("grid", vec![grid()])?;
    assert!(differing(&gridded, &blank, None) > 0, "the grid draws");
    assert_eq!(differing(&gridded, &blank, Some(&BIG)), 0, "an id hides it");
    assert_eq!(
        differing(&gridded, &blank, Some(&UNSELECTABLE)),
        0,
        "an id-less hit hides it too"
    );
    Ok(())
}
