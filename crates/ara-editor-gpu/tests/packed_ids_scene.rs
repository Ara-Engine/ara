//! The Scene overlays composed through the packed id path are, pixel for pixel, the
//! overlays composed through ara-render's own id image: a real raster scene with a
//! selected object, a hovered object, a ground and the grid, a gizmo and an icon on top.
//! `IdSource::Repacked` rewrites ara-render's R32F ids as the RGBA8 form Renderlab
//! delivers on the GPU, then every overlay reads it through the packed path. Metal only:
//! the repack is not wired on Vulkan. Captures go under
//! `target/tmp/ara-editor-gpu-packed-ids-scene`.
#![cfg(target_os = "macos")]
use ara_editor_gpu::{IdSource, NativeUi, Overlay, UiDevice, open_presentation};
use ara_render::{
    Renderer,
    capture::ViewMode,
    scene::{CameraSpec, Material, Mesh, Scene, Texture, Vertex},
};
use ara_ui::paint::{Box2, DisplayList, Draw};
use ara_ui::style::Color;
use ara_ui_render::{atlas::TextMode, gpu::Image, gpu::Renderer as UiRenderer};
use glam::{Mat4, Vec3};

const SIZE: [u32; 2] = [320, 240];
const ENTITY: u32 = 0x7fff_0001;
const GROUPS: u32 = 0x7fff_0002;
const ICON: u32 = 0x7fff_0003;
/// Scene entities, in mesh order (their renderer channels are 1, 2 and 3).
const SELECTED: u64 = 11;
const HOVERED: u64 = 22;
const GROUND: u64 = 33;
const EYE: Vec3 = Vec3::new(0., 2.5, 6.);
const FOV: f32 = 50. * std::f32::consts::PI / 180.;

fn viewport() -> u32 {
    ara_ui::source::stable_id("editor-viewport") as u32 & 0x7fff_ffff
}

/// An axis-aligned box of `size` at `centre`, in world space.
fn cuboid(entity: u64, name: &str, centre: Vec3, size: Vec3) -> Mesh {
    let half = size / 2.;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (normal, u, v) in [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ] {
        let base = vertices.len() as u32;
        for (a, b) in [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)] {
            vertices.push(Vertex {
                position: (centre + (normal + u * a + v * b) * half).into(),
                normal: normal.into(),
                uv: [(a + 1.) / 2., (b + 1.) / 2.],
                tangent: [u.x, u.y, u.z, 1.],
            });
        }
        indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
    }
    Mesh {
        entity,
        name: name.to_owned(),
        vertices,
        indices,
        material: 0,
    }
}

fn scene() -> Scene {
    Scene {
        meshes: vec![
            cuboid(SELECTED, "selected", Vec3::new(-1.2, 0.5, 0.), Vec3::ONE),
            cuboid(HOVERED, "hovered", Vec3::new(1.2, 0.5, 0.), Vec3::ONE),
            cuboid(
                GROUND,
                "ground",
                Vec3::new(0., -0.05, 0.),
                Vec3::new(5., 0.1, 3.),
            ),
        ],
        materials: vec![Material {
            textures: [0; 4],
            ..Default::default()
        }],
        textures: vec![Texture {
            size: [1, 1],
            rgba: vec![255; 4],
            srgb: false,
        }],
        lights: Vec::new(),
        sun_direction: Vec3::new(0.3, 1., 0.5).normalize(),
        sun_lux: Vec3::splat(3_000.),
        environment: vec![[0.2, 0.3, 0.4, 1.]; 8],
        environment_size: [4, 2],
    }
}

/// The pixel a world point lands on in the scene camera.
fn pixel_of(point: Vec3) -> [u32; 2] {
    let aspect = SIZE[0] as f32 / SIZE[1] as f32;
    let clip = Mat4::perspective_rh(FOV, aspect, 0.05, 100.)
        * Mat4::look_at_rh(EYE, Vec3::ZERO, Vec3::Y)
        * point.extend(1.);
    let ndc = clip.truncate() / clip.w;
    [
        ((ndc.x + 1.) / 2. * SIZE[0] as f32) as u32,
        ((1. - ndc.y) / 2. * SIZE[1] as f32) as u32,
    ]
}

struct Rig {
    renderer: Renderer,
    ui: NativeUi,
    paint: UiRenderer,
    output: std::path::PathBuf,
    /// Dropped after the adapter that was made on it.
    _device: UiDevice,
}

impl Rig {
    fn new() -> ara_ui::Result<Self> {
        let target = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
        let output = target.join("ara-editor-gpu-packed-ids-scene");
        std::fs::create_dir_all(&output)?;
        let mut renderer = Renderer::new_raster(scene(), SIZE, &[], output.join("cache"))?;
        renderer.render_view(
            &CameraSpec::LookAt {
                position: EYE,
                target: Vec3::ZERO,
                vertical_fov: FOV,
            },
            1.,
            ViewMode::Lit,
        )?;
        let device = open_presentation(&[])?;
        // SAFETY: `device` is the rig's last field, so it outlives the adapter; the test
        // thread owns every queue, and the renderer outlives the borrowed images' use.
        let mut ui = unsafe { NativeUi::new(&device)? };
        // SAFETY: both images are completed colour images of the renderer's GPU.
        unsafe {
            for (id, image) in [
                (viewport(), renderer.output()),
                (ENTITY, renderer.entity_channel()),
            ] {
                ui.external_image(
                    id,
                    renderer.gpu().native_image(image)?,
                    renderer.gpu().image_view(image)?,
                )?;
            }
        }
        let mut paint = UiRenderer::default();
        paint.external_image(viewport())?;
        paint.external_image(ENTITY)?;
        // The group table the editor builds: texel = channel, red + green the hover
        // group, blue the outline colour group (low 7 bits) and ground (bit 7).
        let mut table = vec![0u8; 4096 * 4];
        for (channel, texel) in [
            (1usize, [1, 0, 1, 255]), // selected: hover group 1, outline colour 1
            (2, [2, 0, 0, 255]),      // hovered: hover group 2, no outline of its own
            (3, [3, 0, 0x80, 255]),   // ground: hover group 3, terrain
        ] {
            table[channel * 4..channel * 4 + 4].copy_from_slice(&texel);
        }
        paint.image(
            GROUPS,
            Image {
                width: 4096,
                height: 1,
                rgba: table,
            },
        )?;
        paint.image(
            ICON,
            Image {
                width: 8,
                height: 8,
                rgba: [255, 255, 255, 200].repeat(64),
            },
        )?;
        let entities: Vec<u64> = renderer.entities().iter().map(|(id, _)| *id).collect();
        assert_eq!(entities, [SELECTED, HOVERED, GROUND], "channels 1, 2, 3");
        Ok(Self {
            renderer,
            ui,
            paint,
            output,
            _device: device,
        })
    }

    /// The frame with `overlays`, reading ids from `ids`.
    fn frame(
        &mut self,
        name: &str,
        ids: IdSource,
        groups: bool,
        overlays: Vec<Overlay>,
    ) -> ara_ui::Result<image::RgbaImage> {
        let rect = Box2 {
            x: 0.,
            y: 0.,
            w: SIZE[0] as f32,
            h: SIZE[1] as f32,
        };
        self.ui.set_dpi(1.)?;
        self.ui.outline_groups(groups.then_some(GROUPS));
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
        // The renderer finished its frame; order the UI's queue after it.
        self.ui.order_after(self.renderer.gpu());
        self.paint
            .render(&mut self.ui, &list, &[], SIZE, TextMode::Raster)?;
        let frame = self.ui.pixels()?;
        frame.save(self.output.join(format!("{name}.png")))?;
        Ok(frame)
    }
}

fn differing(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    a.pixels().zip(b.pixels()).filter(|(a, b)| a != b).count()
}

const ORANGE: [f32; 4] = [1., 0.5, 0., 1.];

/// Selection and hover outlines, the grid, a gizmo and an icon: everything that reads ids
/// or sits over them.
fn everything(hover: [u32; 2]) -> Vec<Overlay> {
    let mut overlays = vec![
        Overlay::Grid {
            camera: [EYE.x, EYE.y, EYE.z, 0.],
            parameters: [(EYE.y / EYE.z).atan(), (FOV / 2.).tan(), 1., 60.],
            color: [0.8, 0.8, 0.8, 0.5],
            axes: [[0.9, 0.2, 0.2, 0.6], [0.2, 0.3, 0.9, 0.6]],
        },
        // Outline colour group 1 (the selection) with the group table, channel 1 without.
        Overlay::Outline {
            index: 1,
            color: ORANGE,
        },
        Overlay::Hover {
            uv: [
                (hover[0] as f32 + 0.5) / SIZE[0] as f32,
                (hover[1] as f32 + 0.5) / SIZE[1] as f32,
            ],
            color: [0.3, 0.8, 1., 1.],
        },
    ];
    // A move gizmo on the selection: three arrows with cones, a plane square, an icon.
    let at = pixel_of(Vec3::new(-1.2, 0.5, 0.)).map(|v| v as f32);
    for (to, color) in [
        ([at[0] + 40., at[1]], [0.9, 0.2, 0.2, 1.]),
        ([at[0], at[1] - 40.], [0.2, 0.8, 0.2, 1.]),
        ([at[0] - 22., at[1] + 22.], [0.2, 0.3, 0.9, 1.]),
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
        rect: [at[0] + 10., at[1] - 36., 16., 16.],
        image: ICON,
        color: [1., 0.9, 0.3, 1.],
    });
    overlays
}

#[test]
fn the_packed_path_composes_the_direct_frame_pixel_for_pixel() -> ara_ui::Result<()> {
    let mut rig = Rig::new()?;
    let hovered = pixel_of(Vec3::new(1.2, 0.5, 0.));
    let selected = pixel_of(Vec3::new(-1.2, 0.5, 0.));
    // The scene is what the test says it is: the pixels hold the objects' channels.
    assert_eq!(rig.renderer.entity_at(hovered)?, Some(HOVERED));
    assert_eq!(rig.renderer.entity_at(selected)?, Some(SELECTED));

    let blank = rig.frame("blank", IdSource::Direct(ENTITY), true, Vec::new())?;
    // Through the editor's group table, and as raw channels.
    for (groups, name) in [(true, "groups"), (false, "channels")] {
        let direct = rig.frame(
            &format!("{name}-direct"),
            IdSource::Direct(ENTITY),
            groups,
            everything(hovered),
        )?;
        let packed = rig.frame(
            &format!("{name}-repacked"),
            IdSource::Repacked(ENTITY),
            groups,
            everything(hovered),
        )?;
        assert!(
            differing(&direct, &blank) > 500,
            "{name}: the overlays draw over the scene"
        );
        assert_eq!(
            differing(&direct, &packed),
            0,
            "{name}: the packed path composes the direct frame (captures in {})",
            rig.output.display()
        );
    }
    // Each id-reading overlay alone reaches the frame and agrees too, so one cannot hide
    // behind the others.
    let one = |rig: &mut Rig, name: &str, overlay: fn() -> Overlay| -> ara_ui::Result<()> {
        let direct = rig.frame(name, IdSource::Direct(ENTITY), true, vec![overlay()])?;
        let packed = rig.frame(name, IdSource::Repacked(ENTITY), true, vec![overlay()])?;
        assert!(differing(&direct, &blank) > 0, "{name} draws");
        assert_eq!(differing(&direct, &packed), 0, "{name}: packed = direct");
        Ok(())
    };
    one(&mut rig, "grid-only", || Overlay::Grid {
        camera: [EYE.x, EYE.y, EYE.z, 0.],
        parameters: [(EYE.y / EYE.z).atan(), (FOV / 2.).tan(), 1., 60.],
        color: [0.8, 0.8, 0.8, 0.5],
        axes: [[0.9, 0.2, 0.2, 0.6], [0.2, 0.3, 0.9, 0.6]],
    })?;
    one(&mut rig, "outline-only", || Overlay::Outline {
        index: 1,
        color: ORANGE,
    })?;
    Ok(())
}
