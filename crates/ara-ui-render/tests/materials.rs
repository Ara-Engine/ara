//! Satin materials on the CPU reference adapter: the same math every GPU adapter runs.
use ara_ui::{
    Result,
    paint::{Box2, DisplayList, Draw},
    style::{Backdrop, Color, Shadow},
};
use ara_ui_render::{CpuGpu, atlas::TextMode, gpu::Renderer};

const SIZE: [u32; 2] = [64, 48];

fn render(commands: Vec<Draw>) -> Result<tiny_skia::Pixmap> {
    let list = DisplayList {
        commands,
        ..DisplayList::default()
    };
    let mut cpu = CpuGpu::default();
    Renderer::default().render(&mut cpu, &list, &[], SIZE, TextMode::Raster)?;
    Ok(cpu.pixmap()?.clone())
}
fn pixel(p: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
    let c = p.pixel(x, y).expect("pixel inside the target");
    [c.red(), c.green(), c.blue(), c.alpha()]
}
fn shadow(inset: bool, offset: [f32; 2], blur: f32, spread: f32) -> Shadow {
    Shadow {
        inset,
        offset,
        blur,
        spread,
        color: Color(255, 255, 255, 255),
    }
}
const BOX: Box2 = Box2 {
    x: 16.,
    y: 12.,
    w: 32.,
    h: 24.,
};

#[test]
fn inset_top_light_is_exactly_one_pixel_inside_the_box() -> Result<()> {
    let p = render(vec![
        Draw::Rect {
            node: 1,
            rect: BOX,
            color: Color(0, 0, 0, 255),
        },
        Draw::Shadow {
            node: 1,
            rect: BOX,
            radius: 0.,
            shadow: shadow(true, [0., 1.], 0., 0.),
        },
    ])?;
    assert_eq!(pixel(&p, 30, 12), [255, 255, 255, 255], "top row lit");
    assert_eq!(pixel(&p, 30, 13), [0, 0, 0, 255], "second row untouched");
    assert_eq!(pixel(&p, 30, 11), [0, 0, 0, 0], "nothing above the box");
    Ok(())
}

#[test]
fn outer_shadow_falls_off_outside_and_never_paints_under_its_box() -> Result<()> {
    let p = render(vec![Draw::Shadow {
        node: 1,
        rect: BOX,
        radius: 6.,
        shadow: shadow(false, [0., 4.], 8., 0.),
    }])?;
    assert_eq!(pixel(&p, 30, 24)[3], 0, "cut out under the box");
    let near = pixel(&p, 30, 38)[3];
    let far = pixel(&p, 30, 45)[3];
    assert!(
        near > far && far > 0,
        "Gaussian falloff: {near} > {far} > 0"
    );
    let ring = render(vec![Draw::Shadow {
        node: 1,
        rect: BOX,
        radius: 0.,
        shadow: shadow(false, [0., 0.], 0., 1.),
    }])?;
    assert_eq!(
        pixel(&ring, 15, 20)[3],
        255,
        "1 px ring outside the left edge"
    );
    assert_eq!(pixel(&ring, 14, 20)[3], 0);
    Ok(())
}

#[test]
fn glass_blurs_and_desaturates_only_what_is_behind_its_shape() -> Result<()> {
    let half = |x: f32, color| Draw::Rect {
        node: 1,
        rect: Box2 {
            x,
            y: 0.,
            w: 32.,
            h: 48.,
        },
        color,
    };
    let glass = |saturate| Draw::Backdrop {
        node: 2,
        rect: BOX,
        radius: 4.,
        backdrop: Backdrop { blur: 3., saturate },
    };
    let edge = render(vec![
        half(0., Color(0, 0, 0, 255)),
        half(32., Color(255, 255, 255, 255)),
        glass(1.),
    ])?;
    let [l, ..] = pixel(&edge, 31, 24);
    let [r, ..] = pixel(&edge, 32, 24);
    assert!(
        (96..=160).contains(&l) && (96..=160).contains(&r),
        "edge blurred: {l} {r}"
    );
    assert_eq!(
        pixel(&edge, 31, 4),
        [0, 0, 0, 255],
        "outside the shape stays sharp"
    );
    let gray = render(vec![
        half(0., Color(255, 0, 0, 255)),
        half(32., Color(255, 0, 0, 255)),
        glass(0.),
    ])?;
    let [r, g, b, _] = pixel(&gray, 30, 24);
    assert!(r == g && g == b, "saturate(0) is gray: {r} {g} {b}");
    Ok(())
}
