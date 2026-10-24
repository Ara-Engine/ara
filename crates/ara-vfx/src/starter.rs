//! Art direction over the reusable sample motion stacks, outside runtime code.
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
fn appearance(
    e: &mut Emitter,
    texture: SpriteTexture,
    blend: Blend,
    alignment: Alignment,
    stretch: f32,
    shutter: f32,
) {
    e.stacks.render.push(item(
        "appearance",
        Module::Appearance {
            texture,
            blend,
            alignment,
            stretch,
            shutter,
        },
    ));
    if !e
        .stacks
        .render
        .iter()
        .any(|m| matches!(m.module, Module::Flipbook { .. }))
    {
        e.stacks.render.push(item(
            "flipbook",
            Module::Flipbook {
                grid: [4, 4],
                fps: 16.,
            },
        ));
    }
}
pub(crate) fn finish(mut effect: Effect) -> Effect {
    match effect.name.as_str() {
        "campfire" => {
            // A waist-high hearth fire: ~0.7 m additive flame tongues, thin sooty
            // smoke that starts above the flame tips, and a few embers. HDR colours
            // are cd/m2 before exposure; overlap stays well below white at dusk.
            let e = &mut effect.emitters[0];
            e.capacity = 512;
            for m in &mut e.stacks.spawn {
                if let Module::Rate { per_second } = &mut m.module {
                    *per_second = 80.;
                }
            }
            for m in &mut e.stacks.initialize {
                match &mut m.module {
                    Module::Lifetime { seconds, size, .. } => {
                        *seconds = 0.85;
                        *size = 0.3;
                    }
                    Module::Sphere { center, radius } => {
                        *center = [0., 0.12, 0.];
                        *radius = 0.2;
                    }
                    Module::Velocity { value, spread } => {
                        *value = [0., 0.95, 0.];
                        *spread = [0.12, 0.2, 0.12];
                    }
                    _ => {}
                }
            }
            for m in &mut e.stacks.update {
                match &mut m.module {
                    Module::CurlNoise { strength, .. } => *strength = 0.55,
                    Module::ColorOverLife { keys } => {
                        *keys = vec![
                            ColorKey {
                                t: 0.,
                                value: [4., 1.6, 0.35, 0.],
                            },
                            ColorKey {
                                t: 0.1,
                                value: [4., 1.5, 0.25, 0.75],
                            },
                            ColorKey {
                                t: 0.55,
                                value: [2.6, 0.65, 0.06, 0.5],
                            },
                            ColorKey {
                                t: 1.,
                                value: [0.4, 0.04, 0., 0.],
                            },
                        ]
                    }
                    Module::SizeOverLife { keys } => {
                        *keys = vec![
                            Key { t: 0., value: 0.24 },
                            Key {
                                t: 0.35,
                                value: 0.34,
                            },
                            Key { t: 1., value: 0.08 },
                        ]
                    }
                    _ => {}
                }
            }
            appearance(
                e,
                SpriteTexture::Fire,
                Blend::Additive,
                Alignment::Camera,
                1.,
                0.,
            );
            e.stacks.render.push(item(
                "hearth_light",
                Module::Light {
                    color: [1., 0.32, 0.055],
                    lumens: 900.,
                    radius: 6.,
                },
            ));
            let mut smoke = crate::samples::smoke().emitters.remove(0);
            smoke.name = "smoke".into();
            for m in &mut smoke.stacks.spawn {
                if let Module::Rate { per_second } = &mut m.module {
                    *per_second = 12.;
                }
            }
            for m in &mut smoke.stacks.initialize {
                match &mut m.module {
                    Module::Lifetime { seconds, .. } => *seconds = 4.5,
                    Module::Cone {
                        origin,
                        height,
                        radius,
                    } => {
                        *origin = [0., 0.55, 0.];
                        *height = 0.2;
                        *radius = 0.12;
                    }
                    Module::Velocity { value, spread } => {
                        *value = [0., 0.55, 0.];
                        *spread = [0.12, 0.15, 0.12];
                    }
                    _ => {}
                }
            }
            for m in &mut smoke.stacks.update {
                match &mut m.module {
                    Module::CurlNoise {
                        frequency,
                        strength,
                    } => {
                        *frequency = 1.2;
                        *strength = 0.35;
                    }
                    Module::SizeOverLife { keys } => {
                        *keys = vec![Key { t: 0., value: 0.25 }, Key { t: 1., value: 1. }]
                    }
                    Module::ColorOverLife { keys } => {
                        *keys = vec![
                            ColorKey {
                                t: 0.,
                                value: [0.1, 0.095, 0.09, 0.],
                            },
                            ColorKey {
                                t: 0.15,
                                value: [0.12, 0.115, 0.11, 0.2],
                            },
                            ColorKey {
                                t: 0.6,
                                value: [0.16, 0.16, 0.16, 0.11],
                            },
                            ColorKey {
                                t: 1.,
                                value: [0.2, 0.2, 0.2, 0.],
                            },
                        ]
                    }
                    _ => {}
                }
            }
            let mut embers = crate::samples::sparks().emitters.remove(0);
            embers.name = "embers".into();
            for m in &mut embers.stacks.spawn {
                if let Module::Rate { per_second } = &mut m.module {
                    *per_second = 12.;
                }
            }
            for m in &mut embers.stacks.initialize {
                if let Module::Velocity { value, spread } = &mut m.module {
                    *value = [0., 2., 0.];
                    *spread = [0.4, 0.7, 0.4];
                }
            }
            effect.emitters.extend([smoke, embers]);
        }
        "sparks" => {
            let e = &mut effect.emitters[0];
            for m in &mut e.stacks.spawn {
                if let Module::Rate { per_second } = &mut m.module {
                    *per_second = 220.;
                }
            }
            for m in &mut e.stacks.initialize {
                if let Module::Lifetime { size, color, .. } = &mut m.module {
                    *size = 0.012;
                    *color = [14., 4.5, 0.7, 0.85];
                }
            }
            e.stacks.update.push(item(
                "cool",
                Module::ColorOverLife {
                    keys: vec![
                        ColorKey {
                            t: 0.,
                            value: [18., 8., 2., 1.],
                        },
                        ColorKey {
                            t: 0.5,
                            value: [9., 1.8, 0.1, 0.8],
                        },
                        ColorKey {
                            t: 1.,
                            value: [1., 0.05, 0., 0.],
                        },
                    ],
                },
            ));
            appearance(
                e,
                SpriteTexture::Spark,
                Blend::Additive,
                Alignment::Velocity,
                3.,
                0.85,
            );
        }
        "magic_swirl" => {
            let e = &mut effect.emitters[0];
            e.capacity = 512;
            for m in &mut e.stacks.spawn {
                if let Module::Rate { per_second } = &mut m.module {
                    *per_second = 24.;
                }
            }
            for m in &mut e.stacks.initialize {
                match &mut m.module {
                    Module::Lifetime { size, .. } => *size = 0.28,
                    Module::Sphere { radius, .. } => *radius = 1.05,
                    _ => {}
                }
            }
            for m in &mut e.stacks.update {
                match &mut m.module {
                    Module::Attractor {
                        strength,
                        softening,
                        ..
                    } => {
                        *strength = 4.;
                        *softening = 0.4;
                    }
                    Module::Vortex { strength, .. } => *strength = 2.4,
                    Module::CurlNoise { strength, .. } => *strength = 0.15,
                    Module::Drag { coefficient } => *coefficient = 1.2,
                    _ => {}
                }
            }
            appearance(
                e,
                SpriteTexture::Wisp,
                Blend::Additive,
                Alignment::Velocity,
                3.5,
                0.5,
            );
            let mut ring = e.clone();
            ring.name = "sigil".into();
            ring.capacity = 256;
            ring.stacks.spawn = vec![item("burst", Module::Burst { tick: 0, count: 1 })];
            ring.stacks.initialize = vec![
                item(
                    "life",
                    Module::Lifetime {
                        seconds: 10.,
                        size: 4.5,
                        color: [0.08, 0.7, 3., 0.5],
                    },
                ),
                item(
                    "ground",
                    Module::Point {
                        position: [0., 0.02, 0.],
                    },
                ),
            ];
            ring.stacks.update.clear();
            ring.stacks.render.clear();
            ring.stacks.render.push(item(
                "sprite",
                Module::Sprite {
                    domain: Domain::Unlit,
                    soft_distance: 0.004,
                    sorting: Sorting::None,
                },
            ));
            appearance(
                &mut ring,
                SpriteTexture::Ring,
                Blend::Additive,
                Alignment::Ground,
                1.,
                0.,
            );
            let base = effect.emitters.remove(0);
            for stream in 0..4 {
                let angle = stream as f32 * std::f32::consts::FRAC_PI_2;
                let x = angle.cos() * 0.8;
                let z = angle.sin() * 0.8;
                let mut orbit = base.clone();
                orbit.name = format!("orbit_{stream}");
                for m in &mut orbit.stacks.initialize {
                    match &mut m.module {
                        Module::Sphere { center, radius } => {
                            *center = [x, 1.4 + angle.sin() * 0.1, z];
                            *radius = 0.06;
                        }
                        Module::Velocity { value, spread } => {
                            *value = [z * 2., 0.05, -x * 2.];
                            *spread = [0.025; 3];
                        }
                        _ => {}
                    }
                }
                effect.emitters.push(orbit);
            }
            effect.emitters.push(ring);
        }
        "smoke" => {
            let e = &mut effect.emitters[0];
            for m in &mut e.stacks.spawn {
                if let Module::Rate { per_second } = &mut m.module {
                    *per_second = 35.;
                }
            }
            for m in &mut e.stacks.update {
                match &mut m.module {
                    Module::SizeOverLife { keys } => {
                        *keys = vec![Key { t: 0., value: 0.45 }, Key { t: 1., value: 1.5 }]
                    }
                    Module::ColorOverLife { keys } => {
                        *keys = vec![
                            ColorKey {
                                t: 0.,
                                value: [0.35, 0.39, 0.44, 0.],
                            },
                            ColorKey {
                                t: 0.15,
                                value: [0.55, 0.58, 0.63, 0.55],
                            },
                            ColorKey {
                                t: 0.75,
                                value: [0.55, 0.58, 0.63, 0.2],
                            },
                            ColorKey {
                                t: 1.,
                                value: [0.55, 0.58, 0.63, 0.],
                            },
                        ]
                    }
                    _ => {}
                }
            }
            appearance(
                e,
                SpriteTexture::Smoke,
                Blend::Alpha,
                Alignment::Camera,
                1.,
                0.,
            );
        }
        "muzzle_flash" => {
            let e = &mut effect.emitters[0];
            for m in &mut e.stacks.spawn {
                if let Module::Burst { count, .. } = &mut m.module {
                    *count = 6;
                }
            }
            for m in &mut e.stacks.initialize {
                match &mut m.module {
                    Module::Lifetime { size, color, .. } => {
                        *size = 0.2;
                        *color = [8., 3., 0.4, 0.8];
                    }
                    Module::Velocity { value, spread } => {
                        *value = [5., 0.25, 0.];
                        *spread = [1.5, 0.5, 0.5];
                    }
                    Module::Cone { height, radius, .. } => {
                        *height = 0.15;
                        *radius = 0.05;
                    }
                    _ => {}
                }
            }
            appearance(
                e,
                SpriteTexture::Fire,
                Blend::Additive,
                Alignment::Velocity,
                1.3,
                0.75,
            );
            e.stacks.update.push(item(
                "flash_envelope",
                Module::ColorOverLife {
                    keys: vec![
                        ColorKey {
                            t: 0.,
                            value: [8., 4., 0.7, 0.],
                        },
                        ColorKey {
                            t: 0.08,
                            value: [12., 6., 1., 1.],
                        },
                        ColorKey {
                            t: 0.25,
                            value: [8., 3., 0.2, 0.8],
                        },
                        ColorKey {
                            t: 1.,
                            value: [0.4, 0.03, 0., 0.],
                        },
                    ],
                },
            ));
        }
        "rain" => {
            let rain = &mut effect.emitters[0];
            for m in &mut rain.stacks.spawn {
                if let Module::Rate { per_second } = &mut m.module {
                    *per_second = 1000.;
                }
            }
            appearance(
                rain,
                SpriteTexture::Spark,
                Blend::Additive,
                Alignment::Velocity,
                2.,
                0.8,
            );
            for m in &mut rain.stacks.render {
                if let Module::EventSpawn { count, .. } = &mut m.module {
                    *count = 1;
                }
            }
            let splash = &mut effect.emitters[1];
            splash.stacks.initialize.truncate(1);
            splash.stacks.update.clear();
            if let Module::Lifetime {
                size,
                color,
                seconds,
            } = &mut splash.stacks.initialize[0].module
            {
                *size = 0.35;
                *seconds = 0.3;
                *color = [0.5, 0.6, 0.7, 0.12];
            }
            splash.stacks.update.push(item(
                "fade",
                Module::ColorOverLife {
                    keys: vec![
                        ColorKey {
                            t: 0.,
                            value: [0.5, 0.6, 0.7, 0.16],
                        },
                        ColorKey {
                            t: 1.,
                            value: [0.5, 0.6, 0.7, 0.],
                        },
                    ],
                },
            ));
            if let Module::Sprite { soft_distance, .. } = &mut splash.stacks.render[0].module {
                *soft_distance = 0.003;
            }
            appearance(
                splash,
                SpriteTexture::Ring,
                Blend::Alpha,
                Alignment::Ground,
                1.,
                0.,
            );
        }
        _ => {}
    }
    effect
}
