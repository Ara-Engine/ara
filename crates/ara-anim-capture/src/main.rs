//! Render actual sampled/solved skeletal pose strips through ara-render.
#[path = "../../ara-anim/examples/v2_support/mod.rs"]
mod support;
use ::glam::{Vec2, Vec3};
use ara_anim::{glam as ag, ik::*, retarget::*, *};
use ara_anim::{
    matching::{FeatureWriter, MatchConfig, MotionDatabase, TrajectoryPoint},
    physical::*,
};
use ara_anim_physics::JoltRagdoll;
use ara_physics::{
    BodyDesc, Collider, PhysicsBackend, PhysicsConfig, PhysicsPose, RigidBody, Shape, Velocity,
};
use ara_physics_jolt::JoltWorld;
use ara_render::{
    Renderer,
    capture::{CaptureRequest, ViewMode},
    scene::*,
};
use std::path::PathBuf;

fn bone(mesh: &mut Mesh, a: Vec3, b: Vec3, width: f32) {
    let delta = b - a;
    if delta.length_squared() < 1e-8 {
        return;
    }
    let side = delta.normalize().any_orthonormal_vector() * width;
    let other = delta.normalize().cross(side);
    let base = mesh.vertices.len() as u32;
    for p in [
        a + side,
        a - side,
        a + other,
        a - other,
        b + side,
        b - side,
        b + other,
        b - other,
    ] {
        mesh.vertices.push(Vertex {
            position: p.to_array(),
            normal: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            tangent: [1.0, 0.0, 0.0, 1.0],
        });
    }
    for face in [
        [0, 4, 2],
        [2, 4, 6],
        [2, 6, 1],
        [1, 6, 5],
        [1, 5, 3],
        [3, 5, 7],
        [3, 7, 0],
        [0, 7, 4],
    ] {
        mesh.indices.extend(face.map(|i| base + i));
    }
}
fn add_pose(
    scene: &mut Scene,
    s: &Skeleton,
    pose: &Pose,
    x: f32,
    y: f32,
    label: &str,
    material: usize,
) -> Result<(), Error> {
    let mut model = vec![ag::Mat4::IDENTITY; s.len()];
    pose::model_matrices(s, pose, &mut model)?;
    let mut mesh = Mesh {
        entity: scene.meshes.len() as u64 + 1,
        name: label.into(),
        vertices: Vec::new(),
        indices: Vec::new(),
        material,
    };
    for (j, data) in s.data().joints.iter().enumerate() {
        if let Some(parent) = data.parent {
            let a = model[parent as usize].w_axis.truncate();
            let b = model[j].w_axis.truncate();
            bone(
                &mut mesh,
                ::glam::Quat::from_rotation_y(0.65) * Vec3::from_array(a.to_array())
                    + Vec3::new(x, y, 0.0),
                ::glam::Quat::from_rotation_y(0.65) * Vec3::from_array(b.to_array())
                    + Vec3::new(x, y, 0.0),
                0.016,
            );
        }
    }
    scene.meshes.push(mesh);
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("crates/ara-anim/captures/v2"));
    let mut scene = Scene {
        meshes: Vec::new(),
        materials: Vec::new(),
        textures: vec![
            Texture {
                size: [1, 1],
                rgba: vec![255; 4],
                srgb: false,
            },
            Texture {
                size: [1, 1],
                rgba: vec![128, 128, 255, 255],
                srgb: false,
            },
        ],
        lights: Vec::new(),
        sun_direction: Vec3::new(-0.3, 0.8, 1.0).normalize(),
        sun_lux: Vec3::splat(3000.0),
        environment: vec![[0.0, 0.0, 0.0, 1.0]],
        environment_size: [1; 2],
    };
    for color in [
        [0.08, 0.65, 0.9, 1.0],
        [0.95, 0.45, 0.08, 1.0],
        [0.3, 0.9, 0.4, 1.0],
        [0.8, 0.3, 0.85, 1.0],
        [1.0, 0.25, 0.25, 1.0],
    ] {
        scene.materials.push(Material {
            base_color: color,
            emissive_roughness: [color[0] * 10.0, color[1] * 10.0, color[2] * 10.0, 0.6],
            ..Default::default()
        });
    }
    let source = support::skeleton(1.0, 20)?;
    let target = support::skeleton(1.3, 20)?;
    let rig = RigMap::auto(&source)?;
    let target_rig = RigMap::auto(&target)?;
    let mut retarget = Retargeter::cook(&source, &rig, &target, &target_rig)?;
    let clips = support::locomotion(&source)?;
    let mut sampler = Sampler::new(&clips[0]);
    let mut input = Pose::new(source.len());
    let mut output_pose = Pose::new(target.len());
    let chain_data = ChainData {
        joints: vec![
            "RightUpperArm".into(),
            "RightLowerArm".into(),
            "RightHand".into(),
        ],
        ..Default::default()
    };
    let mut chain = Chain::cook(&source, &chain_data)?;
    for frame in 0..6 {
        let tick = frame * 6400;
        sampler.sample(&clips[0], &source, tick, &mut input, &mut [])?;
        let root = source.data().motion_root as usize;
        let mut t = input.joint(root).expect("motion root");
        t.translation = ag::Vec3A::ZERO;
        input.set_joint(root, t)?;
        let x = frame as f32 * 1.8 - 4.5;
        add_pose(
            &mut scene,
            &source,
            &input,
            x,
            4.0,
            &format!("walk tick {tick}"),
            0,
        )?;
        chain.solve(
            &source,
            &mut input,
            ag::Vec3::new(0.45, 1.35 + frame as f32 * 0.06, 0.15),
            ChainSolver::Fabrik,
            SolverSettings::default(),
        )?;
        add_pose(
            &mut scene,
            &source,
            &input,
            x,
            1.6,
            &format!("hand FABRIK tick {tick}"),
            1,
        )?;
        retarget.apply(&source, &input, &target, &mut output_pose, true)?;
        add_pose(
            &mut scene,
            &target,
            &output_pose,
            x,
            -1.2,
            &format!("retarget 1.3x tick {tick}"),
            2,
        )?;
    }
    let config = MatchConfig {
        bones: vec!["LeftFoot".into(), "RightFoot".into()],
        horizons: vec![9600, 24000],
        ..Default::default()
    };
    let borrowed: Vec<_> = clips
        .iter()
        .enumerate()
        .map(|(i, c)| (support::reference(i), c))
        .collect();
    let database = MotionDatabase::cook(&config, &source, &borrowed)?;
    let mut features = FeatureWriter::new(&config, &source)?;
    let mut raw = vec![0.0; config.dimensions()];
    let mut normalized = vec![ag::Vec4::ZERO; database.query_groups()];
    for (frame, velocity) in [
        ag::Vec3::Z * 1.2,
        ag::Vec3::Z * 3.2,
        ag::Vec3::X * 1.2,
        -ag::Vec3::X * 1.2,
        -ag::Vec3::Z,
        ag::Vec3::ZERO,
    ]
    .into_iter()
    .enumerate()
    {
        let tick = frame as u32 * 5600;
        let mut before = Pose::new(source.len());
        sampler.sample(
            &clips[0],
            &source,
            tick.saturating_sub(800),
            &mut before,
            &mut [],
        )?;
        sampler.sample(&clips[0], &source, tick, &mut input, &mut [])?;
        let trajectory: Vec<_> = config
            .horizons
            .iter()
            .map(|t| TrajectoryPoint {
                position: velocity * (*t as f32 / 48000.0),
                facing: ag::Vec3::Z,
            })
            .collect();
        features.write(
            &source,
            &input,
            &before,
            1.0 / 60.0,
            velocity,
            &trajectory,
            &mut raw,
        )?;
        database.normalize(&raw, &mut normalized)?;
        let found = database.search(&normalized, 0)?;
        let location = database.frame(found.row).expect("search row");
        let clip = &clips[location.clip];
        let mut sampler = Sampler::new(clip);
        sampler.sample(clip, &source, location.tick, &mut input, &mut [])?;
        let mut root = input.joint(0).expect("root joint");
        root.translation = ag::Vec3A::ZERO;
        root.rotation = ag::Quat::IDENTITY;
        input.set_joint(0, root)?;
        add_pose(
            &mut scene,
            &source,
            &input,
            frame as f32 * 1.8 - 4.5,
            -3.6,
            &format!("motion match clip {} tick {}", location.clip, location.tick),
            3,
        )?;
    }
    let profile = RagdollProfile {
        bones: [
            ("Hips", None),
            ("Chest", Some(0)),
            ("Head", Some(1)),
            ("LeftUpperLeg", Some(0)),
            ("LeftLowerLeg", Some(3)),
            ("RightUpperLeg", Some(0)),
            ("RightLowerLeg", Some(5)),
        ]
        .into_iter()
        .map(|(joint, parent)| RagdollBone {
            joint: joint.into(),
            parent,
            ..Default::default()
        })
        .collect(),
    };
    let mut physics_config = PhysicsConfig {
        threads: 2,
        max_bodies: 128,
        event_capacity: 0,
        ..Default::default()
    };
    physics_config.layer_masks[2] &= !(1 << 2);
    let mut world = JoltWorld::new(physics_config)?;
    let floor = world.backend_mut().create_shape(Shape::Box {
        half_extents: [10.0, 0.5, 10.0],
    })?;
    world.backend_mut().create_bodies(
        &[BodyDesc {
            shape: floor,
            user_data: 1,
            pose: PhysicsPose {
                position: [0.0, -0.5, 0.0],
                ..Default::default()
            },
            body: RigidBody::default(),
            collider: Collider::default(),
            velocity: Velocity::default(),
        }],
        &mut Vec::new(),
    )?;
    let rig = JoltRagdoll::spawn(&mut world, &source, &profile, RootMotion::default(), 10, 2)?;
    let mut physical = PhysicalAnimation::cook(&source, &profile)?;
    let mut state = PhysicalState::default();
    let mut pose = Pose::new(source.len());
    let mut column = 0;
    for tick in 0..=180 {
        if tick == 15 {
            state.request(PhysicalMode::Ragdoll, 12, 1.0)?;
            rig.bind(&mut world)
                .impulse(1, ag::Vec3::X * 1.5, ag::Vec3::Y * 1.3)?;
        }
        if tick == 90 {
            state.request(PhysicalMode::GettingUp, 60, 0.0)?;
        }
        state.tick()?;
        physical.drive(
            &source,
            source.bind_pose(),
            RootMotion::default(),
            &state,
            &mut rig.bind(&mut world),
        )?;
        world.step()?;
        physical.blend(
            &source,
            source.bind_pose(),
            RootMotion::default(),
            &state,
            &rig.bind(&mut world),
            &mut pose,
        )?;
        if [0, 25, 50, 90, 120, 180].contains(&tick) {
            add_pose(
                &mut scene,
                &source,
                &pose,
                column as f32 * 1.8 - 4.5,
                -6.0,
                &format!("Jolt {:?} tick {tick}", state.mode),
                4,
            )?;
            column += 1;
        }
    }
    let size = [1600, 1800];
    let mut renderer = Renderer::new(scene, size, &[], output.join("gpu-cache"))?;
    let request = CaptureRequest {
        camera: CameraSpec::Orthographic {
            centre: Vec2::new(0.0, 0.0),
            world_size: Vec2::new(11.4, 12.825),
        },
        size,
        mode: ViewMode::Albedo,
        deterministic: true,
        exposure: 0.0015,
        settle_frames: 2,
        tick: 0,
        seed: 7,
        marks: Some(30),
    };
    renderer
        .capture(&request)?
        .write(&output.join("pose-strip.png"))?;
    renderer.gpu().assert_clean()?;
    println!("{}", output.join("pose-strip.png").display());
    Ok(())
}
