//! Extension seam for subsystems built on parallel tracks, so they plug in without
//! editing the runtime:
//!
//! - **C# scripts** (`ara-runtime-script`): start the .NET host on the world and add
//!   module types in `prepare`, hand C# component storage to the host in `register`,
//!   run C# systems at [`Stage::Gameplay`]; swap module generations in `before_tick`.
//! - **Networking** (`ara-net`, Listen/Dedicated): drain authenticated arrivals into
//!   the session in `before_tick` (the `ara_sim::SessionAdapter` contract) and
//!   replicate from the read-only world in `after_tick`.
//! - **Audio** (`ara-audio`) and **VFX**: emit from `after_tick` (authoritative events)
//!   and follow the interpolated presentation in `frame`. They never write gameplay.
use crate::{Result, extract::RenderFrame};
use ara_schema::SchemaDb;
use ara_sim::{Session, SimRegistry};
use bevy_ecs::prelude::World;

/// Where a plugin's fixed-tick systems run (see the runtime's system order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// After player input and before AI, navigation, animation and physics.
    Gameplay,
    /// After physics, pose write-back and camera placement; before world poses resolve.
    Late,
}

/// A runtime extension. Every hook has a no-op default. `Sync` so a networked
/// authority can own the runtime (hooks still only run through `&mut self`).
pub trait RuntimePlugin: Send + Sync {
    /// Stable name for diagnostics.
    fn name(&self) -> &'static str;
    /// Prepares the (still empty) simulation world and adds runtime-defined types
    /// (C# modules) to the schema, before the registry is built and the scene projected.
    fn prepare(&mut self, world: &mut World, schema: &mut SchemaDb) -> Result<()> {
        let _ = (world, schema);
        Ok(())
    }
    /// Registers reflected components, resources and commands before projection.
    fn register(&mut self, registry: &mut SimRegistry) -> Result<()> {
        let _ = registry;
        Ok(())
    }
    /// Installs world state, state providers and systems for `stage`. Called once
    /// per stage, in plugin order, while the system order is being built.
    fn install(&mut self, stage: Stage, session: &mut Session) -> Result<()> {
        let _ = (stage, session);
        Ok(())
    }
    /// Runs at each tick boundary before the tick (queue arrivals, swap generations).
    fn before_tick(&mut self, session: &mut Session) -> Result<()> {
        let _ = session;
        Ok(())
    }
    /// Observes a completed tick through the read-only world.
    fn after_tick(&mut self, world: &World) -> Result<()> {
        let _ = world;
        Ok(())
    }
    /// Follows one presented frame (interpolated); never mutates gameplay.
    fn frame(&mut self, world: &World, frame: &RenderFrame) {
        let _ = (world, frame);
    }
}
