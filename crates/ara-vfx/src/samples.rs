//! Six small art-directed effects used by deterministic GPU capture tests.
use crate::*;
use ara_material::Domain;
use std::collections::BTreeMap;

fn item(name: &str, module: Module) -> StackModule {
    StackModule {
        name: name.into(),
        module,
        bindings: BTreeMap::new(),
    }
}
fn emitter(name: &str, capacity: u32, color: [f32; 4], life: f32, size: f32) -> Emitter {
    Emitter {
        name: name.into(),
        capacity,
        bounds: Bounds {
            min: [-8., -2., -8.],
            max: [8., 12., 8.],
        },
        stacks: Stacks {
            initialize: vec![item(
                "lifetime",
                Module::Lifetime {
                    seconds: life,
                    size,
                    color,
                },
            )],
            render: vec![item(
                "sprite",
                Module::Sprite {
                    domain: Domain::Unlit,
                    soft_distance: 0.15,
                    sorting: Sorting::None,
                },
            )],
            ..Stacks::default()
        },
    }
}
fn system(name: &str, emitters: Vec<Emitter>) -> Effect {
    crate::starter::finish(Effect {
        name: name.into(),
        seed: 17,
        hz: 60,
        quality: Quality::default(),
        parameters: vec![],
        emitters,
    })
}
/// Warm flame with rising turbulent particles and an emissive color gradient.
pub fn campfire() -> Effect {
    let mut e = emitter("flame", 4096, [5., 0.6, 0.03, 0.5], 1.4, 0.2);
    e.stacks
        .spawn
        .push(item("rate", Module::Rate { per_second: 1000. }));
    e.stacks.initialize.extend([
        item(
            "hearth",
            Module::Sphere {
                center: [0., 0.2, 0.],
                radius: 0.28,
            },
        ),
        item(
            "rise",
            Module::Velocity {
                value: [0., 1.8, 0.],
                spread: [0.3, 0.5, 0.3],
            },
        ),
    ]);
    e.stacks.update.extend([
        item(
            "curl",
            Module::CurlNoise {
                frequency: 3.,
                strength: 1.5,
            },
        ),
        item("drag", Module::Drag { coefficient: 0.6 }),
        item(
            "fade",
            Module::ColorOverLife {
                keys: vec![
                    ColorKey {
                        t: 0.,
                        value: [4., 0.3, 0.01, 0.25],
                    },
                    ColorKey {
                        t: 0.3,
                        value: [6., 1.4, 0.1, 0.6],
                    },
                    ColorKey {
                        t: 1.,
                        value: [0.3, 0.02, 0., 0.],
                    },
                ],
            },
        ),
        item(
            "shrink",
            Module::SizeOverLife {
                keys: vec![
                    Key { t: 0., value: 0.18 },
                    Key { t: 0.4, value: 0.3 },
                    Key { t: 1., value: 0.02 },
                ],
            },
        ),
    ]);
    system("campfire", vec![e])
}
/// Fast sparks bouncing on a world-space ground plane.
pub fn sparks() -> Effect {
    let mut e = emitter("sparks", 4096, [8., 2.8, 0.25, 0.9], 2., 0.035);
    e.stacks
        .spawn
        .push(item("rate", Module::Rate { per_second: 650. }));
    e.stacks.initialize.extend([
        item(
            "source",
            Module::Point {
                position: [0., 0.7, 0.],
            },
        ),
        item(
            "spray",
            Module::Velocity {
                value: [0., 3., 0.],
                spread: [2.6, 2., 2.6],
            },
        ),
    ]);
    e.stacks.update.extend([
        item(
            "gravity",
            Module::Gravity {
                acceleration: [0., -9.81, 0.],
            },
        ),
        item(
            "floor",
            Module::PlaneCollision {
                normal: [0., 1., 0.],
                offset: 0.,
                restitution: 0.6,
            },
        ),
    ]);
    system("sparks", vec![e])
}
/// A cyan/purple curl-noise vortex with an attraction centre.
pub fn magic_swirl() -> Effect {
    let mut e = emitter("swirl", 8192, [0.3, 1.5, 5., 0.7], 3., 0.045);
    e.stacks
        .spawn
        .push(item("rate", Module::Rate { per_second: 1600. }));
    e.stacks.initialize.extend([
        item(
            "sphere",
            Module::Sphere {
                center: [0., 1.4, 0.],
                radius: 0.85,
            },
        ),
        item(
            "velocity",
            Module::Velocity {
                value: [0., 0.35, 0.],
                spread: [0.1; 3],
            },
        ),
    ]);
    e.stacks.update.extend([
        item(
            "vortex",
            Module::Vortex {
                center: [0., 1.4, 0.],
                axis: [0., 1., 0.],
                strength: 2.2,
            },
        ),
        item(
            "curl",
            Module::CurlNoise {
                frequency: 2.4,
                strength: 0.9,
            },
        ),
        item(
            "pull",
            Module::Attractor {
                position: [0., 1.4, 0.],
                strength: 3.,
                softening: 0.2,
            },
        ),
        item("damping", Module::Drag { coefficient: 1.5 }),
        item(
            "fade",
            Module::ColorOverLife {
                keys: vec![
                    ColorKey {
                        t: 0.,
                        value: [0.1, 2., 6., 0.6],
                    },
                    ColorKey {
                        t: 0.6,
                        value: [1.5, 0.1, 4., 0.7],
                    },
                    ColorKey {
                        t: 1.,
                        value: [0.1, 0., 1., 0.],
                    },
                ],
            },
        ),
    ]);
    system("magic_swirl", vec![e])
}
/// Lit smoke using the shared surface material domain and depth sorting.
pub fn smoke() -> Effect {
    let mut e = emitter("smoke", 2048, [0.2, 0.23, 0.28, 0.14], 4., 0.45);
    e.stacks
        .spawn
        .push(item("rate", Module::Rate { per_second: 180. }));
    e.stacks.initialize.extend([
        item(
            "source",
            Module::Cone {
                origin: [0., 0.1, 0.],
                height: 0.4,
                radius: 0.3,
            },
        ),
        item(
            "rise",
            Module::Velocity {
                value: [0., 0.8, 0.],
                spread: [0.15, 0.2, 0.15],
            },
        ),
    ]);
    e.stacks.update.extend([
        item(
            "curl",
            Module::CurlNoise {
                frequency: 1.5,
                strength: 0.45,
            },
        ),
        item(
            "grow",
            Module::SizeOverLife {
                keys: vec![Key { t: 0., value: 0.3 }, Key { t: 1., value: 1.3 }],
            },
        ),
        item(
            "fade",
            Module::ColorOverLife {
                keys: vec![
                    ColorKey {
                        t: 0.,
                        value: [0.15, 0.17, 0.22, 0.],
                    },
                    ColorKey {
                        t: 0.12,
                        value: [0.25, 0.28, 0.33, 0.16],
                    },
                    ColorKey {
                        t: 1.,
                        value: [0.4, 0.43, 0.48, 0.],
                    },
                ],
            },
        ),
    ]);
    e.stacks.render = vec![item(
        "sprite",
        Module::Sprite {
            domain: Domain::Surface,
            soft_distance: 0.3,
            sorting: Sorting::Depth,
        },
    )];
    system("smoke", vec![e])
}
/// A short burst with one capped emitter light.
pub fn muzzle_flash() -> Effect {
    let mut e = emitter("flash", 256, [15., 5., 0.5, 0.7], 0.12, 0.3);
    e.stacks
        .spawn
        .push(item("burst", Module::Burst { tick: 0, count: 96 }));
    e.stacks.initialize.extend([
        item(
            "cone",
            Module::Cone {
                origin: [0., 0.5, 0.],
                height: 0.8,
                radius: 0.3,
            },
        ),
        item(
            "speed",
            Module::Velocity {
                value: [0., 1., 0.],
                spread: [0.6, 1., 0.6],
            },
        ),
    ]);
    e.stacks.render.extend([
        item(
            "atlas",
            Module::Flipbook {
                grid: [4, 4],
                fps: 80.,
            },
        ),
        item(
            "light",
            Module::Light {
                color: [1., 0.45, 0.08],
                lumens: 900.,
                radius: 3.,
            },
        ),
    ]);
    system("muzzle_flash", vec![e])
}
/// Rain collisions produce next-tick splash particles entirely on the GPU.
pub fn rain() -> Effect {
    let mut e = emitter("rain", 8192, [0.3, 0.6, 0.9, 0.5], 1.2, 0.025);
    e.stacks
        .spawn
        .push(item("rate", Module::Rate { per_second: 1800. }));
    e.stacks.initialize.extend([
        item(
            "cloud",
            Module::Box {
                center: [0., 4., 0.],
                extent: [2., 0.1, 2.],
            },
        ),
        item(
            "fall",
            Module::Velocity {
                value: [0.3, -6., 0.],
                spread: [0.05, 0.5, 0.05],
            },
        ),
    ]);
    e.stacks.update.push(item(
        "floor",
        Module::PlaneCollision {
            normal: [0., 1., 0.],
            offset: 0.01,
            restitution: 0.,
        },
    ));
    e.stacks.render.push(item(
        "splash",
        Module::EventSpawn {
            trigger: Trigger::Collision,
            target: "splash".into(),
            count: 3,
        },
    ));
    let mut splash = emitter("splash", 4096, [0.25, 0.65, 1., 0.5], 0.3, 0.035);
    splash.stacks.initialize.push(item(
        "spray",
        Module::Velocity {
            value: [0., 1.1, 0.],
            spread: [0.7, 0.5, 0.7],
        },
    ));
    splash.stacks.update.push(item(
        "gravity",
        Module::Gravity {
            acceleration: [0., -8., 0.],
        },
    ));
    system("rain", vec![e, splash])
}
/// All capture presets in stable test order, with their representative fixed ticks.
pub fn all() -> Vec<(Effect, u32)> {
    vec![
        (campfire(), 120),
        (sparks(), 100),
        (magic_swirl(), 140),
        (smoke(), 200),
        (muzzle_flash(), 3),
        (rain(), 100),
    ]
}
/// Focused geometry and depth fixtures beyond the six visual baselines.
pub fn additional() -> Vec<(Effect, u32)> {
    let mut ribbon = magic_swirl();
    ribbon.name = "ribbon".into();
    ribbon.emitters[0].stacks.render[0].module = Module::Ribbon {
        width: 0.025,
        domain: Domain::Unlit,
    };
    let mut mesh = sparks();
    mesh.name = "mesh".into();
    mesh.emitters[0].stacks.render[0].module = Module::Mesh {
        triangles: vec![[[-0.5, 0., 0.], [0.5, 0., 0.], [0., 1., 0.]]],
        domain: Domain::Surface,
        sorting: Sorting::Age,
    };
    mesh.emitters[0].stacks.initialize[0].module = Module::Lifetime {
        seconds: 2.,
        size: 0.1,
        color: [3., 1., 0.2, 1.],
    };
    mesh.emitters[0].stacks.initialize[1].module = Module::MeshSurface {
        triangles: vec![[[-0.3, 0.7, -0.3], [0.3, 0.7, -0.3], [0., 0.7, 0.3]]],
    };
    let mut depth = sparks();
    depth.name = "depth_sparks".into();
    depth.emitters[0].stacks.update[1].module = Module::DepthCollision {
        restitution: 0.6,
        thickness: 0.5,
    };
    vec![(ribbon, 100), (mesh, 100), (depth, 100)]
}
/// Million-sprite throughput workload with bounded 1440p coverage.
pub fn million() -> Effect {
    let mut e = emitter("million", 1_000_000, [0.15, 0.4, 1., 0.2], 60., 0.003);
    e.bounds = Bounds {
        min: [-6., -4., -3.],
        max: [6., 7., 3.],
    };
    e.stacks.spawn.push(item(
        "burst",
        Module::Burst {
            tick: 0,
            count: 1_000_000,
        },
    ));
    e.stacks.initialize.extend([
        item(
            "box",
            Module::Box {
                center: [0., 2.5, 0.],
                extent: [3., 1.8, 0.8],
            },
        ),
        item(
            "drift",
            Module::Velocity {
                value: [0., 0.01, 0.],
                spread: [0.01; 3],
            },
        ),
    ]);
    e.stacks.update.push(item(
        "gravity",
        Module::Gravity {
            acceleration: [0., -0.001, 0.],
        },
    ));
    let mut effect = system("million", vec![e]);
    effect.quality.overdraw = 64.;
    effect
}

/// One million 0.05 m sprites, with an explicit 64-layer coverage ceiling.
/// Uses the same motion and distribution as the 0.003 m throughput workload.
pub fn million_fill() -> Effect {
    let mut effect = million();
    effect.name = "million_fill".into();
    effect.quality.overdraw = 64.;
    for m in &mut effect.emitters[0].stacks.initialize {
        if let Module::Lifetime { size, .. } = &mut m.module {
            *size = 0.05;
        }
    }
    effect
}
