//! Command buffers: structural ops (spawn, despawn, insert, remove) and cues that C#
//! appends through `cmd_push` and the host applies at sync points, in order.
//!
//! Each chunk call, behaviour phase and command gets its own buffer, so filling one
//! never needs a lock; buffers are applied in a deterministic order (system order,
//! then chunk order, then push order).

use std::ptr::NonNull;

use ara_diag::Diagnostic;
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::{Entity, EntityAllocator};
use bevy_ecs::prelude::Resource;
use bevy_ecs::ptr::OwningPtr;
use bevy_ecs::world::World;

use crate::abi::{CMD_CUE, CMD_DESPAWN, CMD_INSERT, CMD_REMOVE, CMD_SPAWN};
use crate::codes;

/// One buffered op, as C# pushed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Op {
    pub op: u32,
    pub type_index: u32,
    pub entity: u64,
    pub data: Vec<u8>,
}

/// A buffer of structural ops for one chunk, phase or command.
#[derive(Default)]
pub(crate) struct CmdBuf {
    pub ops: Vec<Op>,
    /// The world's entity allocator for the current run (`reserve_entity` uses it from any thread).
    pub allocator: Option<NonNull<EntityAllocator>>,
}

// SAFETY: a `CmdBuf` is only filled by the one thread running its chunk; the allocator
// pointer refers to bevy's `EntityAllocator`, which is designed for concurrent `alloc`.
unsafe impl Send for CmdBuf {}
// SAFETY: shared references only call `reserve`, which goes through the thread-safe allocator.
unsafe impl Sync for CmdBuf {}

impl CmdBuf {
    /// Clears the buffer for a new run against `allocator`.
    pub fn reset(&mut self, allocator: &EntityAllocator) {
        self.ops.clear();
        self.allocator = Some(NonNull::from(allocator));
    }

    /// Reserves an entity handle (valid immediately; spawned by `CMD_SPAWN`).
    pub fn reserve(&self) -> u64 {
        match self.allocator {
            // SAFETY: the allocator outlives the run this buffer was reset for (it is the
            // world's), and `alloc` takes `&self` and is thread-safe.
            Some(a) => unsafe { a.as_ref() }.alloc().to_bits(),
            None => crate::value::NULL_ENTITY,
        }
    }
}

/// What applying ops needs: component ids and sizes by script index. Lives in the
/// world so system params can apply their buffers at bevy sync points.
#[derive(Resource, Default, Clone)]
pub(crate) struct ScriptCatalog {
    /// `(id, size)` by script component index.
    pub components: Vec<(ComponentId, usize)>,
}

/// A cue emitted by C#, decoded to JSON when the host drains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawCue {
    pub cue: u32,
    pub data: Vec<u8>,
}

/// Cues and problems produced while applying buffers inside the schedule.
#[derive(Resource, Default)]
pub(crate) struct ScriptEvents {
    pub cues: Vec<RawCue>,
    pub problems: Vec<Diagnostic>,
}

/// A structural change that was applied, with what undoing it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// An entity was spawned.
    Spawn {
        /// Entity bits.
        entity: u64,
    },
    /// An entity was despawned; its script-visible components are saved.
    Despawn {
        /// Entity bits.
        entity: u64,
        /// `(script component index, bytes)` it had.
        components: Vec<(u32, Vec<u8>)>,
    },
    /// A component was inserted, replacing `previous` if it had one.
    Insert {
        /// Entity bits.
        entity: u64,
        /// Script component index.
        component: u32,
        /// The bytes it had before, if any.
        previous: Option<Vec<u8>>,
    },
    /// A component was removed.
    Remove {
        /// Entity bits.
        entity: u64,
        /// Script component index.
        component: u32,
        /// The bytes it had.
        bytes: Vec<u8>,
    },
}

/// Inserts raw component bytes; false if the entity is missing. `bytes` must be the
/// component's full layout.
pub(crate) fn insert_bytes(world: &mut World, entity: Entity, id: ComponentId, bytes: &[u8]) -> bool {
    let Ok(mut e) = world.get_entity_mut(entity) else {
        return false;
    };
    // Canonical layouts align to at most 8 bytes, so a u64 buffer is aligned for any component.
    let mut storage = vec![0u64; bytes.len().div_ceil(8).max(1)];
    let dst = storage.as_mut_ptr().cast::<u8>();
    // SAFETY: `storage` holds at least `bytes.len()` bytes and does not overlap `bytes`.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len()) };
    // SAFETY: `dst` comes from a live Vec (non-null), is aligned for the component (see
    // above) and holds an initialized value of its plain-data layout. Script components
    // have no drop glue, and bevy moves the bytes out during the call.
    unsafe { e.insert_by_id(id, OwningPtr::new(NonNull::new_unchecked(dst))) };
    true
}

/// The bytes of a component on an entity, if present.
pub(crate) fn component_bytes(world: &World, entity: Entity, id: ComponentId, size: usize) -> Option<Vec<u8>> {
    let ptr = world.get_entity(entity).ok()?.get_by_id(id).ok()?;
    // SAFETY: the component's storage holds `size` initialized bytes (its layout size),
    // and `&World` keeps it from changing while we copy.
    Some(unsafe { std::slice::from_raw_parts(ptr.as_ptr(), size) }.to_vec())
}

/// Every script-visible component an entity has, as `(index, bytes)`.
pub(crate) fn snapshot(world: &World, catalog: &ScriptCatalog, entity: Entity) -> Vec<(u32, Vec<u8>)> {
    (0u32..)
        .zip(&catalog.components)
        .filter_map(|(i, (id, size))| component_bytes(world, entity, *id, *size).map(|b| (i, b)))
        .collect()
}

/// Applies `ops` in order. Cues go to `cues`, problems to `problems`; when `applied`
/// is given, every change is recorded with what undoing it needs.
pub(crate) fn apply(
    world: &mut World,
    catalog: &ScriptCatalog,
    ops: &[Op],
    cues: &mut Vec<RawCue>,
    problems: &mut Vec<Diagnostic>,
    mut applied: Option<&mut Vec<Applied>>,
) {
    for op in ops {
        if op.op == CMD_CUE {
            cues.push(RawCue { cue: op.type_index, data: op.data.clone() });
            continue;
        }
        let bad = |what: String| {
            Diagnostic::error(
                codes::BAD_DESCRIPTOR,
                format!("command buffer: {what} ({})", crate::value::entity_json(op.entity)),
            )
        };
        let Some(entity) = Entity::try_from_bits(op.entity) else {
            problems.push(bad("invalid entity".into()));
            continue;
        };
        let comp = catalog.components.get(op.type_index as usize).copied();
        match (op.op, comp) {
            (CMD_SPAWN, _) => match world.spawn_empty_at(entity) {
                Ok(_) => {
                    if let Some(a) = applied.as_deref_mut() {
                        a.push(Applied::Spawn { entity: op.entity });
                    }
                }
                Err(e) => problems.push(bad(format!("cannot spawn: {e}"))),
            },
            (CMD_DESPAWN, _) => {
                let components = applied
                    .is_some()
                    .then(|| snapshot(world, catalog, entity));
                if world.despawn(entity) {
                    if let (Some(a), Some(components)) = (applied.as_deref_mut(), components) {
                        a.push(Applied::Despawn { entity: op.entity, components });
                    }
                } else {
                    problems.push(bad("despawn of a missing entity".into()));
                }
            }
            (CMD_INSERT, Some((id, size))) if size == op.data.len() => {
                let previous = applied
                    .is_some()
                    .then(|| component_bytes(world, entity, id, size))
                    .flatten();
                if insert_bytes(world, entity, id, &op.data) {
                    if let Some(a) = applied.as_deref_mut() {
                        a.push(Applied::Insert { entity: op.entity, component: op.type_index, previous });
                    }
                } else {
                    problems.push(bad("insert on a missing entity".into()));
                }
            }
            (CMD_REMOVE, Some((id, size))) => {
                if let Some(bytes) = component_bytes(world, entity, id, size)
                    && let Ok(mut e) = world.get_entity_mut(entity)
                {
                    e.remove_by_id(id);
                    if let Some(a) = applied.as_deref_mut() {
                        a.push(Applied::Remove { entity: op.entity, component: op.type_index, bytes });
                    }
                }
            }
            (CMD_INSERT | CMD_REMOVE, _) => {
                problems.push(bad(format!("unknown component #{} or wrong size", op.type_index)));
            }
            (other, _) => problems.push(bad(format!("unknown op {other}"))),
        }
    }
}

/// Spawns-then-despawns the entities a discarded buffer reserved, so their ids are reused.
pub(crate) fn discard(world: &mut World, ops: &[Op]) {
    for op in ops.iter().filter(|o| o.op == CMD_SPAWN) {
        if let Some(e) = Entity::try_from_bits(op.entity)
            && let Ok(spawned) = world.spawn_empty_at(e)
        {
            spawned.despawn();
        }
    }
}
