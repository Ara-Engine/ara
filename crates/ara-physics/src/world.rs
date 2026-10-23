//! Fixed tick ownership and simulation snapshot seam.
use crate::*;
use bevy_ecs::prelude::Resource;

/// Snapshot at a completed tick, paired with ara-sim's ECS/RNG/input snapshot of that tick.
#[derive(Clone)]
pub struct PhysicsSnapshot<S> {
    tick: u64,
    fixed_dt: f32,
    native: S,
    events: PhysicsEvents,
}
impl<S> PhysicsSnapshot<S> {
    /// Completed fixed tick represented by the snapshot.
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// Trusted native state, e.g. for diagnostics or a backend's byte-size measurement.
    pub fn native(&self) -> &S {
        &self.native
    }
}

/// One isolated fixed-step world. Gameplay state is mirrored to ECS at every boundary.
#[derive(Resource)]
pub struct PhysicsWorld<B: PhysicsBackend> {
    backend: B,
    config: PhysicsConfig,
    tick: u64,
    events: PhysicsEvents,
}
impl<B: PhysicsBackend> PhysicsWorld<B> {
    /// Creates a world with immutable fixed-step/solver/filter configuration.
    pub fn new(config: PhysicsConfig) -> Result<Self> {
        if !config.fixed_dt.is_finite() || config.fixed_dt <= 0.0 {
            return Err(PhysicsError("fixed_dt must be finite and positive".into()));
        }
        let backend = B::new(&config)?;
        Ok(Self {
            backend,
            config,
            tick: 0,
            events: PhysicsEvents::default(),
        })
    }
    /// Native implementation access for shape cooking, spawn, inputs, and queries.
    pub fn backend(&self) -> &B {
        &self.backend
    }
    /// Exclusive boundary access. Never hold this across an ECS query borrow.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
    /// Immutable simulation configuration, including the fixed interval.
    pub fn config(&self) -> &PhysicsConfig {
        &self.config
    }
    /// Number of completed simulation ticks.
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// Effects of the most recently completed tick.
    pub fn events(&self) -> &PhysicsEvents {
        &self.events
    }
    /// Applies body intents in their caller-defined stable order.
    pub fn apply(&mut self, commands: &[BodyCommand]) -> Result<()> {
        self.backend.apply(commands, self.config.fixed_dt)
    }
    /// Runs a single character movement feature with a typed input at this boundary.
    pub fn step_character(&mut self, id: CharacterHandle, input: CharacterInput) -> Result<()> {
        self.backend.step_character(id, input, self.config.fixed_dt)
    }
    /// Completes one fixed tick after all character and vehicle inputs were submitted.
    pub fn step(&mut self) -> Result<()> {
        #[cfg(feature = "profiling")]
        let _profile = ara_profile::scope("physics.step");
        let next = self
            .tick
            .checked_add(1)
            .ok_or_else(|| PhysicsError("physics tick exhausted".into()))?;
        self.backend
            .step(self.config.fixed_dt, next, &mut self.events)?;
        self.tick = next;
        Ok(())
    }
    /// Captures the completed tick; ara-sim stores this beside its ECS snapshot.
    pub fn snapshot(&mut self) -> Result<PhysicsSnapshot<B::Snapshot>> {
        Ok(PhysicsSnapshot {
            tick: self.tick,
            fixed_dt: self.config.fixed_dt,
            native: self.backend.save()?,
            events: self.events.clone(),
        })
    }
    /// Restores a trusted snapshot. Restore the matching ECS state before resimulation.
    /// Structural changes require replay/reconstruction in identical stable-ID order.
    pub fn restore(&mut self, snapshot: &PhysicsSnapshot<B::Snapshot>) -> Result<()> {
        if self.config.fixed_dt.to_bits() != snapshot.fixed_dt.to_bits() {
            return Err(PhysicsError("snapshot fixed interval differs".into()));
        }
        self.backend.restore(&snapshot.native)?;
        self.tick = snapshot.tick;
        self.events.clone_from(&snapshot.events);
        Ok(())
    }
    /// Runs a query individually and returns its first sorted hit.
    pub fn query_first(&self, query: Query) -> Result<Option<QueryHit>> {
        let mut out = QueryResults::default();
        self.backend.queries(&[query], &mut out)?;
        Ok(out.hits.first().copied())
    }
}
