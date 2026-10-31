//! Volumetric fog (look::volumetric) on the native path: a sun shaft through a narrow
//! gap in a roof stays a sharp shaft in the near froxel volume, captures of it are
//! deterministic, and a local fog volume glows only where it is.
use anyhow::{Result, ensure};
use ara_gpu::{Access, GpuDevice, Resource};
use ara_render::{
    Renderer,
    capture::{Capture, CaptureRequest, ViewMode},
    instanced::{
        Geometry, Instance, Lod, Material, Scene, TextureBinding, TextureStats, Textures, Vertex,
    },
    look::{Fog, FogVolume, Look, Quality, VolumetricFog, VolumetricFogQuality},
    scene::CameraSpec,
};
use glam::{Mat4, Quat, Vec3};

struct NoTextures;
impl Textures for NoTextures {
    fn content_digest(&self) -> [u8; 32] {
        [0; 32]
    }
    fn initialize(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn refine(&mut self, _: &mut GpuDevice) -> Result<TextureStats> {
        Ok(TextureStats::default())
    }
    fn binding(&self) -> Option<TextureBinding> {
        None
    }
    fn visit_resources(&self, _: &mut dyn FnMut(Resource, Access)) {}
    fn destroy(self: Box<Self>, _: &mut GpuDevice) -> Result<()> {
        Ok(())
    }
}

/// Width of the gap in the roof the sun shines through, in metres.
const GAP: f32 = 0.25;

/// A 40 m ground, a roof 4 m up with a 2 m long, `GAP`-wide slot along z above the
/// origin, a wall 8 m behind it, and the sun high above, tilted along the slot away
/// from the wall (which it leaves unlit).
fn scene() -> Scene {
    let quad = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(|(x, z)| Vertex {
        position: [x, 0., z],
        normal: [0., 1., 0.],
        tangent: [1., 0., 0., 1.],
        uv: [0.; 2],
        uv1: [0.; 2],
        terrain_weights: 0,
        color: u32::MAX,
    });
    let mut material = Material::default();
    material.pbr.textures = [u32::MAX; 4];
    let instance = |entity: u64, world: Mat4| Instance {
        entity,
        name: format!("quad-{entity}"),
        geometry: 0,
        material: 0,
        world,
        previous_world: world,
        casts_shadows: true,
        paint: None,
        receives_decals: true,
    };
    // The roof (faces up and down, both casting shadows): two halves either side of
    // the slot over 2 m of z, and full-width pieces in front and behind it.
    let roof = |centre: Vec3, size: Vec3, flip: f32| {
        Mat4::from_translation(centre)
            * Mat4::from_quat(Quat::from_rotation_x(flip))
            * Mat4::from_scale(size)
    };
    let pieces = [
        (
            Vec3::new(-(GAP * 0.5 + 10.), 4., 0.),
            Vec3::new(20., 1., 2.),
        ),
        (Vec3::new(GAP * 0.5 + 10., 4., 0.), Vec3::new(20., 1., 2.)),
        (Vec3::new(0., 4., 11.), Vec3::new(41., 1., 20.)),
        (Vec3::new(0., 4., -11.), Vec3::new(41., 1., 20.)),
    ];
    let down = std::f32::consts::PI;
    let wall = Mat4::from_translation(Vec3::new(0., 2., -8.))
        * Mat4::from_quat(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
        * Mat4::from_scale(Vec3::new(40., 1., 8.));
    Scene {
        geometry: vec![Geometry {
            key: [7; 32],
            vertices: quad.to_vec(),
            lods: vec![Lod {
                indices: vec![0, 2, 1, 0, 3, 2],
                error: 0.,
            }],
            bounds: [Vec3::new(-0.5, 0., -0.5), Vec3::new(0.5, 0., 0.5)],
            deforming: false,
            revision: 0,
            vertex_colors: false,
        }],
        instances: [Mat4::from_scale(Vec3::splat(40.)), wall]
            .into_iter()
            .chain(
                pieces
                    .iter()
                    .flat_map(|&(c, s)| [roof(c, s, 0.), roof(c - Vec3::Y * 0.01, s, down)]),
            )
            .enumerate()
            .map(|(i, world)| instance(i as u64 + 1, world))
            .collect(),
        materials: vec![material],
        textures: 0,
        decals: Vec::new(),
        lights: Vec::new(),
        // Tilted along the slot, so the shaft stays a vertical bar across the view.
        sun_direction: Vec3::new(0., 1., -0.2).normalize(),
        sun_lux: Vec3::splat(20000.),
        environment: vec![[0.01, 0.01, 0.01, 1.]],
        environment_size: [1, 1],
    }
}

const SIZE: [u32; 2] = [640, 360];

/// A capture along the slot, level with the middle of the shaft: a vertical bar 8 m
/// away through the image centre, the fog beside it in shadow.
fn capture(renderer: &mut Renderer, name: &str) -> Result<Capture> {
    let capture = renderer.capture(&CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 2., 8.),
            target: Vec3::new(0., 2., 0.),
            vertical_fov: 0.8,
        },
        size: SIZE,
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 0.004,
        settle_frames: 8,
        tick: 0,
        seed: 0,
        marks: None,
    })?;
    capture.write(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-render/volumetric-fog")
            .join(std::process::id().to_string())
            .join(format!("{name}.png")),
    )?;
    Ok(capture)
}

/// Mean luminance (8-bit display values) of the 9x9 pixels around (`x`, `y`).
fn luminance(capture: &Capture, x: u32, y: u32) -> f32 {
    let mut sum = 0.;
    for j in y - 4..=y + 4 {
        for i in x - 4..=x + 4 {
            let p = capture.image.get_pixel(i, j).0;
            sum += 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
        }
    }
    sum / 81.
}

fn renderer(name: &str) -> Result<Renderer> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/volumetric-fog")
        .join(format!("{}-{name}", std::process::id()));
    Renderer::new_native(&scene(), Box::new(NoTextures), SIZE, &[], cache)
}

fn fog() -> Look {
    Look {
        fog: Some(Fog {
            extinction: 0.04,
            height: 1000.,
            albedo: 1.,
            anisotropy: 0.,
            volumetric: Some(VolumetricFog::default()),
            ..Fog::default()
        }),
        ..Look::default()
    }
}

/// The default budgets with a near volume of 4-pixel froxels.
fn near() -> Quality {
    Quality {
        volumetric_fog: Some(VolumetricFogQuality {
            tile: 4,
            slices: 64,
        }),
        ..Quality::default()
    }
}

#[test]
fn a_narrow_sun_shaft_stays_sharp_in_the_near_volume() -> Result<()> {
    let mut renderer = renderer("shaft")?;
    renderer.set_look(fog())?;
    let [cx, cy] = SIZE.map(|v| v / 2);
    // The shaft against the shadowed fog 60 pixels either side.
    let contrast = |c: &Capture| {
        let side = luminance(c, cx - 60, cy).max(luminance(c, cx + 60, cy));
        (luminance(c, cx, cy), side)
    };
    renderer.set_quality(Quality::default())?;
    let (coarse, coarse_side) = contrast(&capture(&mut renderer, "coarse")?);
    renderer.set_quality(near())?;
    let near = capture(&mut renderer, "near")?;
    let (shaft, side) = contrast(&near);
    eprintln!("shaft {shaft} over {side} (coarse grid alone: {coarse} over {coarse_side})");
    ensure!(
        shaft > side + 20. && shaft > 2. * side.max(1.),
        "the shaft stands out of the shadowed fog: {shaft} vs {side}"
    );
    // Deterministic: the same capture again is the same image.
    let again = capture(&mut renderer, "near-again")?;
    ensure!(
        again.image.as_raw() == near.image.as_raw(),
        "volumetric fog captures repeat exactly"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn a_local_fog_volume_glows_only_where_it_is() -> Result<()> {
    let mut renderer = renderer("volume")?;
    renderer.set_quality(near())?;
    renderer.set_look(Look::default())?;
    let [cx, cy] = SIZE.map(|v| v / 2);
    let glow = FogVolume {
        centre: [0., 2., 2.],
        half_extent: [1.; 3],
        density: 0.5,
        emissive: [400.; 3],
        ..FogVolume::default()
    };
    renderer.set_fog_volumes(&[glow])?;
    let lit = capture(&mut renderer, "volume")?;
    let (inside, outside) = (luminance(&lit, cx, cy), luminance(&lit, cx + 200, cy));
    renderer.set_fog_volumes(&[])?;
    let none = luminance(&capture(&mut renderer, "no-volume")?, cx, cy);
    eprintln!("fog volume: {inside} inside, {outside} beside it, {none} without it");
    ensure!(
        inside > none + 40. && outside < none + 5.,
        "the volume glows inside its sphere alone: {inside}, {outside}, {none}"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
