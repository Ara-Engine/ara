//! Extension seam for subsystems built on parallel tracks, so they plug in without
//! editing the runtime:
//!
//! - **C# scripts** (`ara-script`): register components, then add chunk-batched
//!   systems at [`Stage::Gameplay`]; swap module generations in `before_tick`.
//! - **Networking** (`ara-net`, Listen/Dedicated): drain authenticated arrivals into
//!   the session in `before_tick` (the `ara_sim::SessionAdapter` contract) and
//!   replicate from the read-only world in `after_tick`.
//! - **Audio** (`ara-audio`) and **VFX**: emit from `after_tick` (authoritative events)
//!   and follow the interpolated presentation in `frame`. They never write gameplay.
use crate::{Result, extract::RenderFrame};
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

/// A runtime extension. Every hook has a no-op default.
pub trait RuntimePlugin: Send {
    /// Stable name for diagnostics.
    fn name(&self) -> &'static str;
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
