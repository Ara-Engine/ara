//! Shadow patches (look::cascades, look::local): a caster that moves inside cached
//! cascades or local-light faces redraws only the texels it reaches, and those
//! texels match a whole redraw.
use super::*;
use crate::graph::State;
use ara_gpu::{Barrier, Image};

/// A 40 m ground and a 0.6 m caster 1 m above it (instance 1), under a low sun.
fn scene() -> NativeScene {
    let quad =
        [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(|(x, z)| instanced::Vertex {
            position: [x, 0., z],
            normal: [0., 1., 0.],
            tangent: [1., 0., 0., 1.],
            uv: [0.; 2],
            uv1: [0.; 2],
            terrain_weights: 0,
            reserved: 0,
        });
    let mut material = instanced::Material::default();
    material.pbr.textures = [u32::MAX; 4];
    let instance = |entity: u64, world: Mat4| instanced::Instance {
        entity,
        name: format!("quad-{entity}"),
        geometry: 0,
        material: 0,
        world,
        previous_world: world,
    };
    NativeScene {
        geometry: vec![instanced::Geometry {
            key: [83; 32],
            revision: 0,
            deforming: false,
            vertices: quad.to_vec(),
            lods: vec![instanced::Lod {
                indices: vec![0, 2, 1, 0, 3, 2],
                error: 0.,
            }],
            bounds: [Vec3::new(-0.5, 0., -0.5), Vec3::new(0.5, 0., 0.5)],
        }],
        materials: vec![material],
        instances: vec![
            instance(1, Mat4::from_scale(Vec3::splat(40.))),
            instance(2, caster(Vec3::Y)),
        ],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::new(0.4, 1., 0.3).normalize(),
        sun_lux: Vec3::splat(10_000.),
        environment: vec![[0.02, 0.02, 0.02, 1.]],
        environment_size: [1, 1],
    }
}

fn caster(at: Vec3) -> Mat4 {
    Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(0.6))
}

/// Moves the caster to `at` as an exact instance change.
fn move_caster(renderer: &mut Renderer, scene: &mut NativeScene, at: Vec3) -> Result<()> {
    scene.instances[1].world = caster(at);
    renderer.update_native_changed(
        scene,
        &instanced::Changes {
            instances: vec![1],
            ..Default::default()
        },
    )
}

/// Every cascade's depth texels, read back in place.
fn cascade_depths(renderer: &mut Renderer) -> Result<Vec<Vec<f32>>> {
    let targets = {
        let cascades = renderer.look.cascades.as_ref().expect("cascades");
        let size = [cascades.resolution; 2];
        cascades
            .images
            .iter()
            .zip(&cascades.states)
            .map(|(&image, &state)| (image, state, size))
            .collect()
    };
    read_depths(renderer, targets)
}

/// The local-shadow atlas's depth texels, read back in place.
fn atlas_depths(renderer: &mut Renderer) -> Result<Vec<Vec<f32>>> {
    let local = renderer.look.local.as_ref().expect("local shadows");
    let target = (local.atlas, local.state, local.size);
    read_depths(renderer, vec![target])
}

/// D32 `targets` (image, current state, size) read back, their states restored.
fn read_depths(
    renderer: &mut Renderer,
    targets: Vec<(Image, State, [u32; 2])>,
) -> Result<Vec<Vec<f32>>> {
    let mut buffers = Vec::new();
    for &(_, _, [width, height]) in &targets {
        buffers.push(renderer.gpu.create_buffer(
            u64::from(width * height * 4),
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?);
    }
    let mut recorder = renderer.gpu.begin(Queue::Graphics);
    recorder.push(Command::BeginPass("ShadowPatches.Read".into()));
    for (&(source, state, _), &buffer) in targets.iter().zip(&buffers) {
        let image = Resource::Image(source);
        recorder.barriers([
            Barrier {
                resource: image,
                before: state.access,
                after: Access::CopySource,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(buffer),
                before: Access::HostRead,
                after: Access::CopyDestination,
                handoff: None,
            },
        ]);
        recorder.push(Command::ReadImage {
            source,
            destination: buffer,
        });
        recorder.barriers([
            Barrier {
                resource: image,
                before: Access::CopySource,
                after: state.access,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(buffer),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
        ]);
    }
    recorder.push(Command::EndPass);
    let point = renderer.gpu.submit(recorder.finish(), &[])?;
    renderer.gpu.resolve(point)?;
    let mut depths = Vec::new();
    for buffer in buffers {
        let bytes = renderer.gpu.read(buffer)?;
        depths.push(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
        );
        renderer.gpu.retire(Resource::Buffer(buffer))?;
    }
    renderer.gpu.collect()?;
    Ok(depths)
}

/// Texels that differ by more than `tolerance` between two reads of the same maps.
fn differing(a: &[Vec<f32>], b: &[Vec<f32>], tolerance: f32) -> usize {
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            a.iter()
                .zip(b)
                .filter(|(a, b)| (*a - *b).abs() > tolerance)
                .count()
        })
        .sum()
}

/// A renderer over `scene` at `quality`, three live frames in (caches drawn).
fn settled(
    name: &str,
    scene: &NativeScene,
    quality: crate::look::Quality,
    camera: &CameraSpec,
) -> Result<Renderer> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/shadow-patches")
        .join(name)
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let mut renderer = Renderer::new_native(
        scene,
        Box::new(EmptyTextures),
        [128, 96],
        &[],
        root.join("gpu"),
    )?;
    renderer.set_quality(quality)?;
    for _ in 0..3 {
        renderer.render_view(camera, 1., ViewMode::Lit)?;
    }
    Ok(renderer)
}

const CAMERA: CameraSpec = CameraSpec::LookAt {
    position: Vec3::new(0., 4., 8.),
    target: Vec3::ZERO,
    vertical_fov: 1.,
};

#[test]
fn a_moved_caster_patches_cascades_to_what_a_whole_redraw_draws() -> Result<()> {
    let mut scene = scene();
    let quality = crate::look::Quality {
        shadows: crate::look::Shadows {
            cascades: 2,
            resolution: 512,
            distance: 30.,
            far: None,
        },
        ..crate::look::Quality::default()
    };
    let mut renderer = settled("cascades", &scene, quality, &CAMERA)?;
    let before = cascade_depths(&mut renderer)?;

    move_caster(&mut renderer, &mut scene, Vec3::new(0.4, 1., 0.2))?;
    let patched = renderer.look.cascades.as_ref().expect("cascades").patched;
    renderer.render_view(&CAMERA, 1., ViewMode::Lit)?;
    assert_eq!(
        renderer.look.cascades.as_ref().expect("cascades").patched,
        patched + 2,
        "both cascades hold the caster and patch it"
    );
    let patch = cascade_depths(&mut renderer)?;
    let moved = differing(&before, &patch, 1e-4);
    assert!(moved > 0, "the caster's shadow moved");

    // Forget both maps: the first redraws whole at once, the coarse one next frame.
    renderer
        .look
        .cascades
        .as_mut()
        .expect("cascades")
        .observe(true, &[]);
    for _ in 0..2 {
        renderer.render_view(&CAMERA, 1., ViewMode::Lit)?;
    }
    let whole = cascade_depths(&mut renderer)?;
    // The cropped transform rounds depths differently (a few ULPs), never coverage.
    let off = differing(&patch, &whole, 1e-6);
    println!("cascade patches: {moved} texels moved, {off} differ from a whole redraw");
    assert_eq!(off, 0, "patched texels match a whole redraw");
    renderer.gpu.assert_clean()?;
    Ok(())
}

#[test]
fn a_moved_caster_patches_a_local_face_to_what_a_whole_redraw_draws() -> Result<()> {
    let mut scene = scene();
    scene.sun_lux = Vec3::ZERO;
    // A point light 2 m above the caster: it shadows the ground through the -Y face.
    scene.lights.push(crate::scene::Light {
        position_range: [0., 3., 0., 10.],
        color_lumens: [1., 1., 1., 2000.],
        direction_outer: [0., -1., 0., 0.],
        parameters: [0., 1., 0., 0.],
    });
    let quality = crate::look::Quality {
        local_shadows: Some(crate::look::LocalShadows {
            lights: 1,
            resolution: 256,
            redraws: 6,
            in_gi_and_fog: false,
        }),
        ..crate::look::Quality::default()
    };
    let mut renderer = settled("local", &scene, quality, &CAMERA)?;
    let before = atlas_depths(&mut renderer)?;

    move_caster(&mut renderer, &mut scene, Vec3::new(0.3, 1., 0.2))?;
    let patched = |renderer: &Renderer| renderer.look.local.as_ref().expect("local").cache.patched;
    let count = patched(&renderer);
    renderer.render_view(&CAMERA, 1., ViewMode::Lit)?;
    assert_eq!(
        patched(&renderer),
        count + 1,
        "the -Y face holds the caster"
    );
    let patch = atlas_depths(&mut renderer)?;
    let moved = differing(&before, &patch, 1e-4);
    assert!(moved > 0, "the caster's shadow moved");

    // Forget every face: all six redraw whole within the budget.
    renderer
        .look
        .local
        .as_mut()
        .expect("local")
        .cache
        .observe(true, &[]);
    renderer.render_view(&CAMERA, 1., ViewMode::Lit)?;
    let whole = atlas_depths(&mut renderer)?;
    let off = differing(&patch, &whole, 1e-6);
    println!("local patches: {moved} texels moved, {off} differ from a whole redraw");
    assert_eq!(off, 0, "patched texels match a whole redraw");
    renderer.gpu.assert_clean()?;
    Ok(())
}
