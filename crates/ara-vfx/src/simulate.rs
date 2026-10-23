use crate::cpu_modules::{initialize, update};
use crate::{Bounds, Effect, Emitter, Module, Sorting, Trigger, error};
use ara_diag::Diagnostic;
use glam::{Vec3, Vec4};

/// Deterministic integer mixing shared with Slang, independent of platform RNG.
pub fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^ (x >> 16)
}
/// A particle snapshot; SoA storage is authoritative, this value is a short borrow copy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    /// World-space position in metres.
    pub position: Vec3,
    /// World-space velocity in metres per second.
    pub velocity: Vec3,
    /// Current age in seconds; negative indicates a dead slot.
    pub age: f32,
    /// Lifetime in seconds.
    pub life: f32,
    /// Straight scene-linear RGBA.
    pub color: Vec4,
    /// World-space diameter.
    pub size: f32,
    /// Immutable per-spawn random seed.
    pub seed: u32,
    /// Fixed tick at which this particle was born.
    pub birth: u32,
}
impl Particle {
    pub(crate) fn new(seed: u32, birth: u32) -> Self {
        Self {
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            age: 0.,
            life: 1.,
            color: Vec4::ONE,
            size: 0.1,
            seed,
            birth,
        }
    }
}
/// CPU collision provider for gameplay-authoritative depth effects.
pub trait DepthQuery {
    /// Return a contact point and outward normal, or None when no surface is hit.
    fn contact(&self, previous: Vec3, position: Vec3, thickness: f32) -> Option<(Vec3, Vec3)>;
}
/// An event from the CPU path; callers may use it for authoritative gameplay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// Source emitter index in authored order.
    pub emitter: u32,
    /// Stable particle storage slot within that emitter.
    pub slot: u32,
    /// Death or collision.
    pub trigger: Trigger,
    /// World-space event location, copied before slot reuse.
    pub position: Vec3,
    /// Tick when the event occurred.
    pub tick: u32,
}
/// Structure-of-arrays particle component storage, with stable alive/dead lists.
pub struct ParticleSoa {
    /// Position and age channels.
    pub position_age: Vec<[f32; 4]>,
    /// Velocity and lifetime channels.
    pub velocity_life: Vec<[f32; 4]>,
    /// Straight scene-linear RGBA channel.
    pub color: Vec<[f32; 4]>,
    /// Diameter, seed bits and birth tick bits.
    pub size_seed_birth: Vec<[u32; 4]>,
    /// Live slots in ascending storage order, compacted each tick.
    pub alive: Vec<u32>,
    /// Free slots in ascending storage order.
    pub dead: Vec<u32>,
}
impl ParticleSoa {
    fn new(capacity: u32) -> Self {
        Self {
            position_age: vec![[0., 0., 0., -1.]; capacity as usize],
            velocity_life: vec![[0.; 4]; capacity as usize],
            color: vec![[0.; 4]; capacity as usize],
            size_seed_birth: vec![[0; 4]; capacity as usize],
            alive: Vec::with_capacity(capacity as usize),
            dead: (0..capacity).collect(),
        }
    }
    /// Read a live or dead slot; invalid indices return None.
    pub fn particle(&self, slot: u32) -> Option<Particle> {
        let i = slot as usize;
        let p = self.position_age.get(i)?;
        let v = *self.velocity_life.get(i)?;
        let c = *self.size_seed_birth.get(i)?;
        Some(Particle {
            position: Vec3::from_slice(p),
            age: p[3],
            velocity: Vec3::from_slice(&v),
            life: v[3],
            color: Vec4::from(*self.color.get(i)?),
            size: f32::from_bits(c[0]),
            seed: c[1],
            birth: c[2],
        })
    }
    fn put(&mut self, slot: usize, p: Particle) {
        self.position_age[slot] = p.position.extend(p.age).to_array();
        self.velocity_life[slot] = p.velocity.extend(p.life).to_array();
        self.color[slot] = p.color.to_array();
        self.size_seed_birth[slot] = [p.size.to_bits(), p.seed, p.birth, 0];
    }
}
/// Agent-readable deterministic counts; GPU timing data is added by ara-vfx-gpu.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct SimulationStats {
    /// Completed fixed ticks.
    pub tick: u32,
    /// Total live particles after update.
    pub alive: u32,
    /// Spawns omitted because an emitter's capacity was exhausted.
    pub dropped_spawns: u64,
    /// Events omitted because the per-emitter quality cap was exhausted.
    pub dropped_events: u64,
}
/// Deterministic CPU path. Store this as an ECS component; events are authoritative.
#[derive(bevy_ecs::prelude::Component)]
pub struct CpuSystem {
    effect: Effect,
    particles: Vec<ParticleSoa>,
    events: Vec<Event>,
    next_events: Vec<Event>,
    stats: SimulationStats,
}
impl CpuSystem {
    /// Resolve default parameter bindings and allocate bounded SoA components.
    pub fn new(effect: &Effect) -> Result<Self, Diagnostic> {
        let effect = effect.resolve(&Default::default())?;
        let particles = effect
            .capacities()
            .into_iter()
            .map(ParticleSoa::new)
            .collect();
        Ok(Self {
            effect,
            particles,
            events: Vec::new(),
            next_events: Vec::new(),
            stats: SimulationStats::default(),
        })
    }
    /// Read-only particle channels for rendering or gameplay queries.
    pub fn emitters(&self) -> &[ParticleSoa] {
        &self.particles
    }
    /// Events produced by the most recent tick, ordered by emitter, slot, trigger.
    pub fn events(&self) -> &[Event] {
        &self.events
    }
    /// Current agent-readable counters.
    pub fn stats(&self) -> &SimulationStats {
        &self.stats
    }
    /// Advance exactly one fixed tick; no elapsed wall-clock time enters this path.
    pub fn step(&mut self, depth: Option<&dyn DepthQuery>) -> Result<(), Diagnostic> {
        if depth.is_none()
            && self.effect.emitters.iter().any(|e| {
                e.stacks
                    .update
                    .iter()
                    .any(|m| matches!(m.module, Module::DepthCollision { .. }))
            })
        {
            return Err(error(
                "depth collision on CPU requires an authoritative DepthQuery",
            ));
        }
        let tick = self.stats.tick;
        if tick == u32::MAX {
            return Err(error("VFX tick exhausted; reset this instance"));
        }
        let dt = 1. / self.effect.hz as f32;
        self.next_events.clear();
        for (index, (e, soa)) in self
            .effect
            .emitters
            .iter()
            .zip(&mut self.particles)
            .enumerate()
        {
            let requested = spawn_demand(e, tick, self.effect.hz);
            let mut emitted = 0u32;
            let mut demand = u64::from(requested);
            let capacity = soa.position_age.len() as u32;
            let mut spawn = |origin: Option<Vec3>| {
                if let Some(slot) = soa.dead.get(emitted as usize).copied() {
                    let seed =
                        hash(self.effect.seed ^ hash(index as u32) ^ hash(tick) ^ hash(emitted));
                    let mut p = Particle::new(seed, tick);
                    let mut rng = seed;
                    for item in &e.stacks.initialize {
                        initialize(&item.module, &mut p, &mut rng);
                    }
                    if let Some(origin) = origin {
                        p.position += origin;
                    }
                    soa.put(slot as usize, p);
                    emitted += 1;
                }
            };
            for _ in 0..requested.min(capacity) {
                spawn(None);
            }
            let mut routed = 0u32;
            for event in &self.events {
                for output in &self.effect.emitters[event.emitter as usize].stacks.render {
                    if let Module::EventSpawn {
                        trigger,
                        target,
                        count,
                    } = &output.module
                        && *trigger == event.trigger
                        && target == &e.name
                    {
                        demand += u64::from(*count);
                        for _ in 0..*count {
                            if routed < self.effect.quality.events {
                                spawn(Some(event.position));
                                routed += 1;
                            } else {
                                self.stats.dropped_events += 1;
                            }
                        }
                    }
                }
            }
            self.stats.dropped_spawns += demand.saturating_sub(u64::from(emitted));
            soa.alive.clear();
            soa.dead.clear();
            for slot in 0..soa.position_age.len() {
                let mut p = soa
                    .particle(slot as u32)
                    .expect("slot is bounded by all SoA channel lengths");
                if p.age >= 0. {
                    let previous = p.position;
                    p.age += dt;
                    p.position += p.velocity * dt;
                    let mut collided = false;
                    for item in &e.stacks.update {
                        collided |= update(&item.module, &mut p, previous, dt, depth);
                    }
                    if collided {
                        self.next_events.push(Event {
                            emitter: index as u32,
                            slot: slot as u32,
                            trigger: Trigger::Collision,
                            position: p.position,
                            tick,
                        });
                    }
                    if p.age >= p.life {
                        self.next_events.push(Event {
                            emitter: index as u32,
                            slot: slot as u32,
                            trigger: Trigger::Death,
                            position: p.position,
                            tick,
                        });
                        p.age = -1.;
                    }
                    soa.put(slot, p);
                }
                if p.age >= 0. {
                    soa.alive.push(slot as u32);
                } else {
                    soa.dead.push(slot as u32);
                }
            }
        }
        std::mem::swap(&mut self.events, &mut self.next_events);
        self.stats.tick += 1;
        self.stats.alive = self.particles.iter().map(|s| s.alive.len() as u32).sum();
        Ok(())
    }
    /// Capture state at a fixed tick by replaying from a new instance.
    pub fn at_tick(
        effect: &Effect,
        tick: u32,
        depth: Option<&dyn DepthQuery>,
    ) -> Result<Self, Diagnostic> {
        let mut system = Self::new(effect)?;
        for _ in 0..tick {
            system.step(depth)?;
        }
        Ok(system)
    }
}
/// Emission demand from fixed-tick rate accumulation and exact bursts.
pub fn spawn_demand(e: &Emitter, tick: u32, hz: u32) -> u32 {
    e.stacks.spawn.iter().fold(0u32, |sum, m| {
        sum.saturating_add(match m.module {
            Module::Rate { per_second } => {
                (((f64::from(tick) + 1.) * f64::from(per_second) / f64::from(hz)).floor()
                    - (f64::from(tick) * f64::from(per_second) / f64::from(hz)).floor())
                    as u32
            }
            Module::Burst { tick: at, count } if at == tick => count,
            _ => 0,
        })
    })
}
/// Bounding-box/frustum support without reading GPU simulation state.
pub fn bounds_visible(bounds: Bounds, view_projection: glam::Mat4) -> bool {
    let mut corners = [Vec4::ZERO; 8];
    for (i, c) in corners.iter_mut().enumerate() {
        *c = view_projection
            * Vec3::new(
                if i & 1 == 0 {
                    bounds.min[0]
                } else {
                    bounds.max[0]
                },
                if i & 2 == 0 {
                    bounds.min[1]
                } else {
                    bounds.max[1]
                },
                if i & 4 == 0 {
                    bounds.min[2]
                } else {
                    bounds.max[2]
                },
            )
            .extend(1.);
    }
    !(0..6).any(|plane| {
        corners.iter().all(|c| match plane {
            0 => c.x < -c.w,
            1 => c.x > c.w,
            2 => c.y < -c.w,
            3 => c.y > c.w,
            4 => c.z < 0.,
            _ => c.z > c.w,
        })
    })
}
/// Geometry output sorting policy for an emitter.
pub fn sorting(emitter: &Emitter) -> Sorting {
    emitter
        .stacks
        .render
        .iter()
        .find_map(|m| match m.module {
            Module::Sprite { sorting, .. } | Module::Mesh { sorting, .. } => Some(sorting),
            Module::Ribbon { .. } => Some(Sorting::Age),
            _ => None,
        })
        .unwrap_or(Sorting::None)
}
