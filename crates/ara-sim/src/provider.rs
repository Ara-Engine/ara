//! External subsystem state: physics, navigation and animation keep native handles,
//! caches or non-schema runtime values outside the registered component set. A
//! provider owns those types, captures their portable state into snapshots/hashes and
//! rebuilds or restores them on restore. Nothing here runs during an ordinary tick.
use crate::Result;
use bevy_ecs::{component::ComponentId, prelude::World};
use serde_json::Value;
use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    sync::Arc,
};

/// Trusted in-process checkpoint data (for example a native physics save). Never
/// serialized: portable snapshots restore through [`StateProvider::restore`] instead.
pub type TrustedState = Arc<dyn Any + Send + Sync>;

/// A subsystem checkpoint provider, installed with `Session::add_provider`.
///
/// Its owned component and resource types are exempt from the registered-state check,
/// so it must capture everything future-affecting that they hold. Portable state is
/// stored in [`crate::Snapshot`] and included in state hashes; use stable identity
/// ordering so equal worlds capture equal values.
pub trait StateProvider: Send + Sync + 'static {
    /// Stable unique name, used as the snapshot key.
    fn name(&self) -> &'static str;
    /// Component and resource types this provider owns (captured, derived or rebuilt).
    fn owned_types(&self) -> Vec<TypeId>;
    /// Runtime-registered components and resources it owns that have no Rust type
    /// (C# module columns), by their current IDs in `world`.
    fn owned_components(&self, world: &World) -> Vec<ComponentId> {
        let _ = world;
        Vec::new()
    }
    /// Portable deterministic state at a completed tick. Return `Value::Null` when
    /// registered ECS mirrors already hold everything that affects the future.
    fn capture(&self, world: &World) -> Result<Value>;
    /// Restores portable state after registered components and resources are restored,
    /// rebuilding derived native state from the restored ECS values.
    fn restore(&self, world: &mut World, state: &Value) -> Result<()>;
    /// Optional exact native checkpoint for in-process rollback, taken before capture.
    fn checkpoint(&self, world: &mut World) -> Result<Option<TrustedState>> {
        let _ = world;
        Ok(None)
    }
    /// Restores a checkpoint taken by this provider in this process.
    fn restore_checkpoint(
        &self,
        world: &mut World,
        state: &Value,
        trusted: &TrustedState,
    ) -> Result<()> {
        let _ = trusted;
        self.restore(world, state)
    }
}

/// Trusted provider checkpoints carried by an in-process [`crate::Snapshot`].
#[derive(Clone, Default)]
pub(crate) struct Trusted(pub BTreeMap<String, TrustedState>);

impl std::fmt::Debug for Trusted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

/// Owned type IDs of every installed provider.
pub(crate) fn owned(providers: &[Box<dyn StateProvider>]) -> Vec<TypeId> {
    providers.iter().flat_map(|p| p.owned_types()).collect()
}

pub(crate) fn owned_components(
    providers: &[Box<dyn StateProvider>],
    world: &World,
) -> Vec<ComponentId> {
    providers
        .iter()
        .flat_map(|p| p.owned_components(world))
        .collect()
}
