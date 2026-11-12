//! Ara's VFX graph effects on Renderlab's GPU particle emitters.
//!
//! Each emitter of an effect becomes one Renderlab emitter, placed and scaled by its
//! `VisualEffect`. Mapped: rate and a burst at the first tick; lifetime, point, sphere,
//! box and cone spawns; velocity with per-axis variation; gravity, drag, curl noise, one
//! vortex and one attractor; a bounce plane; colour and size curves of up to four keys;
//! lit and unlit, alpha-blended and additive camera-facing sprites with soft edges; and
//! emitter lights, as point lights. Everything else is reported once per module kind
//! (`ARA_RENDERLAB_UNSUPPORTED`) and left out or approximated as the report says.
//!
//! The simulation is Renderlab's. It steps by the frame's elapsed simulation ticks (Ara's
//! 60 Hz effect clock), so it is deterministic in what it spawns but not particle for
//! particle Ara's. A new effect starts warmed by 2.5 seconds, as Ara's composite warms
//! every effect before its first frame (`WARM_TICKS`).
use crate::rl::Engine;
use crate::{gaps::Gaps, texture};
use anyhow::Result;
use ara_material::Domain;
use ara_runtime::{
    components::LightKind,
    extract::{EffectView, LightView, RenderFrame},
};
use ara_vfx::{Blend, ColorKey, Effect, Emitter, Key, Module, SpriteTexture};
use renderlab::{
    asset::TextureRole,
    engine::{
        ColorKey as RlColorKey, EmitterDescriptor, EmitterId, Flipbook, MAX_CURVE_KEYS,
        MAX_PARTICLES, PlaneCollision, SizeKey, SpawnShape, SpriteAlignment, TextureId, Vec3,
    },
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Ticks per second of the library's effects, and the clock the bridge steps them by.
const EFFECT_HZ: f32 = 60.;
/// Ticks stepped at most per frame.
const MAX_CATCH_UP: u64 = 8;
/// Ticks every new effect runs before its first frame, whatever the scene tick, so
/// smoke and fire appear in their steady state.
const WARM_TICKS: u32 = 150;
/// Seconds a new effect is warmed before it is first drawn (`WARM_TICKS`).
const WARM_SECONDS: f32 = WARM_TICKS as f32 / EFFECT_HZ;
const FNV: u64 = 0xcbf2_9ce4_8422_2325;
/// Ara colours fire by a heat ramp of its sheet's density, from dark red to a hot white
/// (`render.slang`). The ramp's brightest channel is 1.6: the sheet stores it over 1.6
/// and fire emitters' radiance is scaled up by the same amount.
const FIRE_RAMP: [[f32; 3]; 2] = [[0.7, 0.08, 0.005], [1.6, 1.1, 0.35]];
const FIRE_RAMP_PEAK: f32 = 1.6;

fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A Renderlab emitter, the placement it was last set from and its light.
struct Live {
    id: EmitterId,
    key: u64,
    light: Option<LightView>,
}

/// One Ara emitter mapped: Renderlab's descriptor with the curves it borrows, and the
/// emitter's light.
struct Mapped {
    desc: EmitterDescriptor<'static>,
    colors: Vec<RlColorKey>,
    sizes: Vec<SizeKey>,
    light: Option<LightView>,
    /// The sprite sheet's atlas family (`ara_vfx::atlas::family`).
    sheet: Option<u32>,
}

impl Mapped {
    fn descriptor(&self, prewarm: f32) -> EmitterDescriptor<'_> {
        EmitterDescriptor {
            color_keys: &self.colors,
            size_keys: &self.sizes,
            prewarm,
            ..self.desc
        }
    }
}

/// Renderlab emitters for the frame's effects.
#[derive(Default)]
pub(crate) struct Effects {
    /// By effect entity and emitter index, in that order (the lights' order).
    live: BTreeMap<(u64, usize), Live>,
    /// Library effects by name, resolved once; `None` for an unknown name.
    library: HashMap<String, Option<Effect>>,
    /// The simulation tick last stepped to.
    tick: Option<u64>,
    /// Sprite sheets uploaded, by atlas family.
    sheets: HashMap<u32, TextureId>,
}

impl Effects {
    /// Emitter lights of the effects last synced, to light the scene with.
    pub fn lights(&self) -> impl Iterator<Item = &LightView> {
        self.live.values().filter_map(|live| live.light.as_ref())
    }

    /// Makes Renderlab's emitters follow `frame.effects` and steps them by the ticks
    /// elapsed since the last frame (none for a redraw of the same tick, so stills and
    /// captures stand still).
    ///
    /// The engine ticks only when a simulation tick elapsed, on the first frame, or when
    /// `posed` says a skinned object's pose changed: a tick that steps nothing still
    /// restarts the engine's per-tick work (the atmosphere's environment is placed again,
    /// which relights the GI cache), so redrawing the same tick from a moving camera must
    /// not tick.
    pub fn sync(
        &mut self,
        engine: &mut Engine,
        frame: &RenderFrame,
        posed: bool,
        gaps: &mut Gaps,
    ) -> Result<()> {
        let first = self.tick.is_none();
        let elapsed = self
            .tick
            .map_or(0, |last| frame.tick.saturating_sub(last).min(MAX_CATCH_UP));
        self.tick = Some(frame.tick);
        // The particle step and the shared per-tick work (skinning) that every view rendered
        // after it reuses.
        if first || elapsed > 0 || posed {
            engine.tick(elapsed as f32 / EFFECT_HZ)?;
        }
        let prewarm = WARM_SECONDS;

        let mut seen = HashSet::new();
        for view in &frame.effects {
            let effect = self.library.entry(view.effect.clone()).or_insert_with(|| {
                ara_vfx::samples::find(&view.effect)
                    .and_then(|effect| effect.resolve(&BTreeMap::new()).ok())
            });
            let Some(effect) = effect else {
                gaps.unsupported(
                    "VFX effects outside the cooked library",
                    &format!("`{}` is not drawn", view.effect),
                );
                continue;
            };
            if effect.hz as f32 != EFFECT_HZ {
                gaps.unsupported(
                    "VFX effects not at 60 ticks per second",
                    "they step at 60 per second",
                );
            }
            let key = placement_key(view);
            for (index, emitter) in effect.emitters.iter().enumerate() {
                let slot = (view.entity, index);
                // Mapped only when the emitter starts or its placement changes.
                if self.live.get(&slot).is_some_and(|live| live.key == key) {
                    seen.insert(slot);
                    continue;
                }
                let mapped = map(emitter, effect.seed.wrapping_add(index as u32), view, gaps);
                let Some(mapped) = mapped else { continue };
                seen.insert(slot);
                let sprite = match mapped.sheet {
                    Some(family) => Some(sheet(&mut self.sheets, engine, family)?),
                    None => None,
                };
                match self.live.get_mut(&slot) {
                    Some(live) => {
                        engine.update_emitter(live.id, &mapped.descriptor(0.), sprite)?;
                        (live.key, live.light) = (key, mapped.light);
                    }
                    None => {
                        let id = engine.create_emitter(&mapped.descriptor(prewarm), sprite)?;
                        let light = mapped.light;
                        self.live.insert(slot, Live { id, key, light });
                    }
                }
            }
        }
        let gone: Vec<_> = self
            .live
            .keys()
            .filter(|slot| !seen.contains(slot))
            .copied()
            .collect();
        for slot in gone {
            if let Some(live) = self.live.remove(&slot) {
                engine.remove_emitter(live.id)?;
            }
        }
        Ok(())
    }
}

/// The Renderlab texture of one procedural sprite sheet, uploaded on first use into
/// `sheets`, cooked from `ara_vfx::atlas` on that first use.
fn sheet(
    sheets: &mut HashMap<u32, TextureId>,
    engine: &mut Engine,
    family: u32,
) -> Result<TextureId> {
    if let Some(id) = sheets.get(&family) {
        return Ok(*id);
    }
    let side = ara_vfx::atlas::SHEET_SIZE;
    let bytes = (side * side * 4) as usize;
    let sheet = ara_vfx::atlas::cook_sheet(family);
    // Density is the opacity; the colour comes from the emitter, times fire's heat ramp
    // (the G/B lighting gradients are left out: lit sprites shade flat). Ara reads each
    // 128-pixel frame with v up the image, Renderlab with v down it: frames flip in place.
    let fire = ara_vfx::atlas::family(SpriteTexture::Fire) == Some(family);
    let tint = |p: &[u8; 4]| -> [u8; 4] {
        if !fire {
            return [255, 255, 255, p[3]];
        }
        let heat = (f32::from(p[0]) / 255.).powf(0.6);
        let [lo, hi] = FIRE_RAMP;
        let c =
            |i: usize| texture::linear_to_srgb((lo[i] + (hi[i] - lo[i]) * heat) / FIRE_RAMP_PEAK);
        [c(0), c(1), c(2), p[3]]
    };
    let (side, tile) = (side as usize, side as usize / 4);
    let mut rgba = Vec::with_capacity(bytes);
    for y in 0..side {
        let flipped = (y / tile) * tile + (tile - 1 - y % tile);
        let row = &sheet[flipped * side * 4..(flipped + 1) * side * 4];
        rgba.extend(row.as_chunks::<4>().0.iter().flat_map(tint));
    }
    let id = engine.create_texture(texture::chain(
        [side as u32; 2],
        rgba,
        TextureRole::BaseColorSrgb,
    ))?;
    sheets.insert(family, id);
    Ok(id)
}

/// Everything about an effect's placement that reaches its emitters.
fn placement_key(view: &EffectView) -> u64 {
    let mut key = fnv(FNV, view.effect.as_bytes());
    for value in view.position.iter().chain([&view.scale, &view.intensity]) {
        key = fnv(key, &value.to_bits().to_le_bytes());
    }
    key
}

/// Up to `MAX_CURVE_KEYS` of a curve's keys: all of them, or the first, the last and
/// those between at evenly spaced positions (reported).
fn curve<T: Copy>(keys: &[T], gaps: &mut Gaps, what: &'static str) -> Vec<T> {
    if keys.len() <= MAX_CURVE_KEYS {
        return keys.to_vec();
    }
    gaps.unsupported(what, "the curve keeps four of its keys");
    let last = keys.len() - 1;
    (0..MAX_CURVE_KEYS)
        .map(|i| keys[i * last / (MAX_CURVE_KEYS - 1)])
        .collect()
}

/// A curve as the linear keys Renderlab's emitters take: its own when every segment is
/// straight, else [`MAX_CURVE_KEYS`] samples of it across the particle's life.
fn linear_keys(curve: &ara_vfx::Curve) -> Vec<Key> {
    if curve
        .keys
        .iter()
        .all(|k| k.interpolation == ara_vfx::ara_curve::Interpolation::Linear)
    {
        return curve
            .keys
            .iter()
            .map(|k| Key {
                t: k.time,
                value: k.value,
            })
            .collect();
    }
    (0..MAX_CURVE_KEYS)
        .map(|i| {
            let t = i as f32 / (MAX_CURVE_KEYS - 1) as f32;
            Key {
                t,
                value: curve.evaluate(t),
            }
        })
        .collect()
}

/// Renderlab's emitter for one Ara emitter placed by `view`, or `None` when it spawns
/// nothing Renderlab can draw (no rate or burst, or no lifetime).
fn map(emitter: &Emitter, seed: u32, view: &EffectView, gaps: &mut Gaps) -> Option<Mapped> {
    let scale = view.scale.max(0.);
    let origin = Vec3::from_array(view.position);
    let place = |p: [f32; 3]| origin + Vec3::from_array(p) * scale;
    let mut desc = EmitterDescriptor {
        position: origin,
        spawn_radius: 0.,
        spawn_rate: 0.,
        velocity: Vec3::ZERO,
        velocity_spread: 0.,
        acceleration: Vec3::ZERO,
        drag: 0.,
        fade: [0.; 2],
        emission: [[0.; 3]; 2],
        soft_distance: 0.,
        seed: seed.max(1),
        ..Default::default()
    };
    // Straight linear RGBA and world diameter, constant unless curves replace them.
    let (mut colour, mut size) = ([1f32; 4], 0.1f32);
    let (mut colour_keys, mut size_keys): (Vec<ColorKey>, Vec<Key>) = (Vec::new(), Vec::new());
    let (mut lit, mut additive, mut lifetime, mut light) = (false, false, None, None);
    let mut sheet = None;
    let (mut vortices, mut attractors, mut curls) = (0, 0, 0);
    let stacks = &emitter.stacks;
    let modules = stacks
        .spawn
        .iter()
        .chain(&stacks.initialize)
        .chain(&stacks.update)
        .chain(&stacks.render);
    for stacked in modules {
        match &stacked.module {
            Module::Rate { per_second } => desc.spawn_rate += per_second.max(0.),
            Module::Burst { tick, count } => {
                if *tick > 0 {
                    gaps.unsupported(
                        "VFX bursts after the first tick",
                        "the burst spawns when the effect starts",
                    );
                }
                desc.burst += count;
            }
            Module::Lifetime {
                seconds,
                size: diameter,
                color,
            } => {
                lifetime = Some(seconds.max(1e-3));
                size = *diameter;
                colour = *color;
            }
            Module::Point { position } => {
                desc.position = place(*position);
                desc.spawn_shape = SpawnShape::Sphere;
                desc.spawn_radius = 0.;
            }
            Module::Sphere { center, radius } => {
                desc.position = place(*center);
                desc.spawn_shape = SpawnShape::Sphere;
                desc.spawn_radius = radius.max(0.) * scale;
            }
            Module::Box { center, extent } => {
                desc.position = place(*center);
                desc.spawn_shape = SpawnShape::Box {
                    half_extent: Vec3::from_array(*extent).max(Vec3::ZERO) * scale,
                };
            }
            Module::Cone {
                origin,
                height,
                radius,
            } => {
                desc.position = place(*origin);
                desc.spawn_shape = SpawnShape::Cone {
                    axis: Vec3::new(0., height * scale, 0.),
                };
                desc.spawn_radius = radius.max(0.) * scale;
            }
            Module::MeshSurface { .. } => gaps.unsupported(
                "VFX mesh-surface spawn shapes",
                "particles spawn at the effect's origin",
            ),
            Module::Velocity { value, spread } => {
                desc.velocity = Vec3::from_array(*value) * scale;
                desc.velocity_box = Vec3::from_array(*spread).abs() * scale;
            }
            Module::Gravity { acceleration } => {
                desc.acceleration += Vec3::from_array(*acceleration) * scale;
            }
            // Ara's rational drag, v / (1 + c dt), is exp(-c dt) to first order.
            Module::Drag { coefficient } => desc.drag += coefficient.max(0.),
            Module::CurlNoise {
                frequency,
                strength,
            } => {
                curls += 1;
                // A field over the effect's scaled space: wavelengths and pull scale with it.
                desc.forces.curl = [frequency.max(0.) / scale.max(1e-6), strength * scale];
                // As Ara's: sampled in the effect's space, phased 0.013 rad per birth tick.
                // Renderlab's revision on Apple predates the curl's origin and phase
                // (Renderlab #25); emitters are refused there anyway.
                #[cfg(not(target_vendor = "apple"))]
                {
                    desc.forces.curl_origin = origin;
                    desc.forces.curl_phase_rate = 0.013 * EFFECT_HZ;
                }
                #[cfg(target_vendor = "apple")]
                let _ = origin;
            }
            Module::Vortex {
                center,
                axis,
                strength,
            } => {
                vortices += 1;
                desc.forces.vortex_center = place(*center);
                desc.forces.vortex_axis = Vec3::from_array(*axis).normalize_or_zero() * *strength;
            }
            Module::Attractor {
                position,
                strength,
                softening,
            } => {
                attractors += 1;
                // Scaling distances by s scales the pull by s³ and the softening by s².
                desc.forces.attractor = place(*position);
                desc.forces.attractor_strength = [
                    strength * scale.powi(3),
                    (softening * scale * scale).max(1e-6),
                ];
            }
            Module::ColorOverLife { keys } => colour_keys = keys.clone(),
            // Renderlab takes linear keys: a curve's own when straight, else samples of it.
            Module::SizeOverLife { curve } => size_keys = linear_keys(curve),
            Module::PlaneCollision {
                normal,
                offset,
                restitution,
            } => {
                // Ara's plane, dot(n, p) = offset in the effect's space, in world space.
                let n = Vec3::from_array(*normal).normalize_or_zero();
                desc.collision = (n != Vec3::ZERO).then(|| PlaneCollision {
                    normal: n,
                    offset: offset * scale + n.dot(origin),
                    restitution: restitution.clamp(0., 1.),
                });
            }
            Module::DepthCollision { .. } => gaps.unsupported(
                "VFX depth collisions",
                "particles pass through the scene's surfaces",
            ),
            Module::Sprite {
                domain,
                soft_distance,
                ..
            } => {
                lit = matches!(domain, Domain::Surface);
                desc.soft_distance = soft_distance.max(0.) * scale;
            }
            Module::Appearance {
                texture,
                blend,
                alignment,
                stretch,
                shutter,
            } => {
                additive = *blend == Blend::Additive;
                sheet = ara_vfx::atlas::family(*texture);
                // As Ara's composite: sheets play their sixteen frames once over each
                // life, flames stand twice as tall as wide, smoke turns at random.
                desc.flipbook = sheet.map(|_| Flipbook {
                    grid: [4, 4],
                    fps: 0.,
                    over_life: true,
                });
                desc.aspect = if *texture == SpriteTexture::Fire { 2. } else { 1. };
                desc.random_rotation = *texture == SpriteTexture::Smoke;
                if *texture == SpriteTexture::Smoke && lit {
                    gaps.unsupported(
                        "VFX smoke sprite lighting gradients",
                        "lit smoke shades flat across each sprite",
                    );
                }
                desc.alignment = match alignment {
                    ara_vfx::Alignment::Camera => SpriteAlignment::Camera,
                    ara_vfx::Alignment::Velocity => SpriteAlignment::Velocity {
                        stretch: stretch.max(1.),
                    },
                    ara_vfx::Alignment::Ground => SpriteAlignment::Plane { normal: Vec3::Y },
                };
                if *alignment == ara_vfx::Alignment::Camera && *stretch > 1. {
                    gaps.unsupported(
                        "VFX stretched camera-facing sprites",
                        "they are drawn unstretched",
                    );
                }
                if *alignment == ara_vfx::Alignment::Velocity && *shutter > 0. {
                    gaps.unsupported(
                        "VFX velocity shutter (streak length from speed)",
                        "streaks keep their authored stretch at any speed",
                    );
                }
            }
            Module::Flipbook { .. } => gaps.unsupported(
                "VFX flipbook material atlases",
                "the procedural sprite sheet plays over each life; material textures are not sampled",
            ),
            Module::Ribbon { .. } | Module::Mesh { .. } => gaps.unsupported(
                "VFX ribbon and mesh particles",
                "those emitters draw camera-facing sprites",
            ),
            Module::Light {
                color,
                lumens,
                radius,
            } => {
                // As Ara's composite: at the bounds' centre, 0.7 m up.
                let b = &emitter.bounds;
                let at = [
                    (b.min[0] + b.max[0]) * 0.5,
                    0.7,
                    (b.min[2] + b.max[2]) * 0.5,
                ];
                light = Some(LightView {
                    kind: LightKind::Point,
                    position: place(at).to_array(),
                    color: *color,
                    lumens: lumens.max(0.) * view.intensity.max(0.),
                    range: radius.max(0.) * scale,
                    ..Default::default()
                });
            }
            Module::EventSpawn { .. } => gaps.unsupported(
                "VFX event spawns",
                "death and collision events spawn nothing",
            ),
        }
    }
    if curls > 1 || vortices > 1 || attractors > 1 {
        gaps.unsupported(
            "VFX emitters with more than one curl-noise, vortex or attractor module",
            "the last of each kind acts",
        );
    }
    let lifetime = lifetime?;
    if desc.spawn_rate <= 0. && desc.burst == 0 {
        return None;
    }
    desc.lifetime = [lifetime; 2];
    // A ring of every live particle; a higher rate would retire particles early.
    let wanted = desc.spawn_rate * lifetime + desc.burst as f32;
    let capacity = emitter.capacity.clamp(1, MAX_PARTICLES);
    if wanted > capacity as f32 {
        gaps.limit(
            "2,048 particles per emitter",
            "busier emitters spawn at the rate that capacity sustains",
        );
        desc.burst = desc.burst.min(capacity);
        desc.spawn_rate = (capacity - desc.burst) as f32 / lifetime;
    }
    desc.capacity = (wanted.ceil() as u32).clamp(1, capacity);
    let unit = |v: f32| if v.is_finite() { v.clamp(0., 1.) } else { 0. };
    let radiance = |v: f32| if v.is_finite() { v.max(0.) } else { 0. };
    // Lit sprites scatter the scene's light: colour is albedo, alpha coverage. Unlit colour
    // is radiance, premultiplied by alpha; additive sprites do not cover what is behind.
    let boost = if sheet.is_some() && sheet == ara_vfx::atlas::family(SpriteTexture::Fire) {
        FIRE_RAMP_PEAK
    } else {
        1.
    };
    let shade = |rgba: [f32; 4]| -> ([f32; 4], [f32; 3]) {
        if lit {
            (rgba.map(unit), [0.; 3])
        } else {
            let alpha = unit(rgba[3]);
            (
                [0., 0., 0., if additive { 0. } else { alpha }],
                std::array::from_fn(|i| radiance(rgba[i] * alpha * view.intensity * boost)),
            )
        }
    };
    let (color, emission) = shade(colour);
    desc.color = [color; 2];
    desc.emission = [emission; 2];
    desc.size = [size.max(0.) * scale; 2];
    // Ara raises a flame by a quarter of its size, so its doubled height grows upward.
    if desc.aspect > 1. {
        desc.position.y += 0.25 * desc.size[0];
    }
    let colors = curve(
        &colour_keys,
        gaps,
        "VFX colour gradients with more than four keys",
    )
    .into_iter()
    .map(|k| {
        let (color, emission) = shade(k.value);
        RlColorKey {
            t: k.t.clamp(0., 1.),
            color,
            emission,
        }
    })
    .collect();
    let sizes = curve(&size_keys, gaps, "VFX size curves with more than four keys")
        .into_iter()
        .map(|k| SizeKey {
            t: k.t.clamp(0., 1.),
            size: k.value.max(0.) * scale,
        })
        .collect();
    Some(Mapped {
        desc,
        colors,
        sizes,
        light,
        sheet,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn view(effect: &str, scale: f32) -> EffectView {
        EffectView {
            entity: 7,
            label: "effect".into(),
            effect: effect.into(),
            position: [10., 0., -4.],
            scale,
            intensity: 2.,
        }
    }

    fn library(name: &str) -> Effect {
        ara_vfx::samples::find(name)
            .unwrap()
            .resolve(&BTreeMap::new())
            .unwrap()
    }

    /// A flame like the library's campfire, with known numbers.
    fn flame() -> Emitter {
        use ara_vfx::{Bounds, Sorting, StackModule, Stacks};
        let item = |name: &str, module| StackModule::new(name, module);
        let key = |t, value| ColorKey { t, value };
        Emitter {
            name: "flame".into(),
            capacity: 4096,
            bounds: Bounds {
                min: [-8.; 3],
                max: [8.; 3],
            },
            stacks: Stacks {
                spawn: vec![item("rate", Module::Rate { per_second: 1000. })],
                initialize: vec![
                    item(
                        "lifetime",
                        Module::Lifetime {
                            seconds: 1.4,
                            size: 0.2,
                            color: [5., 0.6, 0.03, 0.5],
                        },
                    ),
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
                ],
                update: vec![
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
                                key(0., [4., 0.3, 0.01, 0.25]),
                                key(0.3, [6., 1.4, 0.1, 0.6]),
                                key(1., [0.3, 0.02, 0., 0.]),
                            ],
                        },
                    ),
                    item(
                        "shrink",
                        Module::SizeOverLife {
                            curve: ara_vfx::Curve::linear(&[(0., 0.18), (0.4, 0.3), (1., 0.02)]),
                        },
                    ),
                ],
                render: vec![item(
                    "sprite",
                    Module::Sprite {
                        domain: Domain::Unlit,
                        soft_distance: 0.15,
                        sorting: Sorting::None,
                    },
                )],
            },
        }
    }

    #[test]
    fn a_flame_maps_its_shape_motion_curves_and_unlit_colour() {
        let mut gaps = Gaps::default();
        let m = map(&flame(), 3, &view("campfire", 2.), &mut gaps).unwrap();
        let d = m.descriptor(0.);
        // 1000 per second for 1.4 s fits the ring.
        assert_eq!(
            (d.spawn_rate, d.capacity, d.lifetime),
            (1000., 1400, [1.4; 2])
        );
        // The hearth sphere, placed and scaled with the effect.
        assert_eq!(d.position, Vec3::new(10., 0.4, -4.));
        assert!((d.spawn_radius - 0.56).abs() < 1e-6);
        assert_eq!(d.velocity, Vec3::new(0., 3.6, 0.));
        assert_eq!(d.velocity_box, Vec3::new(0.6, 1., 0.6));
        assert_eq!(d.drag, 0.6);
        // Curl noise over the doubled space: half the frequency, twice the pull.
        assert_eq!(d.forces.curl, [1.5, 3.]);
        // Unlit: no albedo; each key's radiance is its colour times alpha and intensity.
        assert_eq!(d.color_keys.len(), 3);
        assert_eq!(&d.color_keys[1].color[..3], &[0.; 3]);
        assert!((d.color_keys[1].emission[0] - 6. * 0.6 * 2.).abs() < 1e-4);
        let sizes: Vec<f32> = d.size_keys.iter().map(|k| k.size).collect();
        assert_eq!(sizes, [0.36, 0.6, 0.04]);
    }

    #[test]
    fn sparks_bounce_on_the_ground_plane_in_world_space() {
        let mut gaps = Gaps::default();
        let sparks = library("sparks");
        let m = map(&sparks.emitters[0], 1, &view("sparks", 1.), &mut gaps).unwrap();
        let plane = m.desc.collision.unwrap();
        assert_eq!(plane.normal, Vec3::Y);
        // The effect sits at y = 0, so the plane keeps its offset.
        assert!((plane.offset - 0.).abs() < 1e-6, "{plane:?}");
    }

    #[test]
    fn the_muzzle_flash_bursts_from_a_cone_and_lights_the_scene() {
        let mut gaps = Gaps::default();
        let flash = library("muzzle_flash");
        let m = map(&flash.emitters[0], 1, &view("muzzle_flash", 1.), &mut gaps).unwrap();
        let bursts: u32 = flash.emitters[0]
            .stacks
            .spawn
            .iter()
            .map(|m| match m.module {
                Module::Burst { count, .. } => count,
                _ => 0,
            })
            .sum();
        assert!(bursts > 0);
        assert_eq!(m.desc.burst, bursts);
        assert!(matches!(m.desc.spawn_shape, SpawnShape::Cone { axis } if axis.y > 0.));
        let light = m.light.unwrap();
        assert!(light.lumens > 0. && light.range > 0.);
        // Its sheet plays once over each life.
        assert!(m.sheet.is_some() && m.desc.flipbook.is_some_and(|f| f.over_life));
    }

    #[test]
    fn every_library_effect_maps_within_renderlabs_limits() {
        for effect in ara_vfx::samples::library() {
            let effect = effect.resolve(&BTreeMap::new()).unwrap();
            let mut gaps = Gaps::default();
            for emitter in &effect.emitters {
                let Some(m) = map(emitter, 1, &view(&effect.name, 1.), &mut gaps) else {
                    continue;
                };
                let d = m.descriptor(1.);
                assert!((1..=MAX_PARTICLES).contains(&d.capacity), "{}", effect.name);
                let held = d.spawn_rate * d.lifetime[1] + d.burst as f32;
                assert!(held <= d.capacity as f32 + 1., "{}", effect.name);
                assert!(
                    d.color_keys.len() <= MAX_CURVE_KEYS && d.size_keys.len() <= MAX_CURVE_KEYS
                );
            }
        }
    }
}
