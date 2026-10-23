//! Direct typed Local dispatch; encoding is only requested by snapshots/recording.
use crate::{BoundaryCommand, Result};
use ara_input::PlayerId;
use ara_schema::Schema;
use bevy_ecs::prelude::World;
use serde::{Serialize, de::DeserializeOwned};

/// Authenticated command identity/target, independent of any transport representation.
#[derive(Debug, Clone)]
pub struct CommandContext {
    /// Explicit input owner, assigned by a trusted session adapter.
    pub player: PlayerId,
    /// Monotonic per-owner intent identity.
    pub sequence: u64,
    /// Stable target address, when required.
    pub target: Option<String>,
}

/// A simulation command's data and execution shared by Local, authority and replay.
/// The Local path moves this value straight to execute, with no Serde calls.
pub trait SimCommand: Schema + Serialize + DeserializeOwned + Send + Sync {
    /// Stable registry/replay command name.
    const NAME: &'static str;
    /// Checks typed bounds without serialization before queueing. Gameplay preconditions
    /// that depend on current state must also be checked by execute before mutation.
    fn validate(&self, context: &CommandContext) -> Result<()> {
        let _ = context;
        Ok(())
    }
    /// Executes at a fixed boundary. All authoritative writes belong in ECS state.
    fn execute(self, world: &mut World, context: &CommandContext) -> Result<()>;
}

pub(crate) trait QueuedTyped: Send + Sync {
    fn context(&self) -> &CommandContext;
    fn record(&self) -> Result<BoundaryCommand>;
    fn execute(self: Box<Self>, world: &mut World) -> Result<()>;
}

pub(crate) struct Typed<C> {
    pub context: CommandContext,
    pub value: C,
}

impl<C: SimCommand> QueuedTyped for Typed<C> {
    fn context(&self) -> &CommandContext {
        &self.context
    }
    fn record(&self) -> Result<BoundaryCommand> {
        Ok(BoundaryCommand {
            kind: C::NAME.into(),
            player: self.context.player,
            sequence: self.context.sequence,
            target: self.context.target.clone(),
            payload: serde_json::to_value(&self.value)?,
        })
    }
    fn execute(self: Box<Self>, world: &mut World) -> Result<()> {
        self.value.execute(world, &self.context)
    }
}

pub(crate) enum Pending {
    Data(BoundaryCommand),
    Typed(Box<dyn QueuedTyped>),
}

impl Pending {
    pub fn key(&self) -> (PlayerId, u64) {
        match self {
            Self::Data(c) => (c.player, c.sequence),
            Self::Typed(c) => (c.context().player, c.context().sequence),
        }
    }
}
