//! Component registry and the op vocabulary. The registry stands in for the A2 schema DB: one
//! typed entry per component drives JSON get/set, the binary snapshot columns and the state hash.
//! Ops are the single mutation path for live patches in the player and "Keep changes" in the host.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail};
use bevy_ecs::component::Component;
use bevy_ecs::component::Mutable;
use bevy_ecs::entity::Entity;
use bevy_ecs::world::World;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{IdIndex, Label, NextSceneId, Orbit, SceneId, Tint, Transform};

/// Implemented by every component that crosses the play boundary.
pub trait PlayComponent:
    Component<Mutability = Mutable> + Serialize + DeserializeOwned + Clone + PartialEq
{
    /// Name used in scene files, op paths and snapshot columns.
    const NAME: &'static str;
    /// Appends the canonical little-endian byte form.
    fn write(&self, out: &mut Vec<u8>);
    /// Reads one value written by [`PlayComponent::write`], advancing `input`.
    fn read(input: &mut &[u8]) -> Result<Self>;
}

/// Splits `n` bytes off the front of `input`.
pub fn take<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if input.len() < n {
        bail!("snapshot truncated: need {n} bytes, have {}", input.len());
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

macro_rules! pod_component {
    ($ty:ty, $name:literal) => {
        impl PlayComponent for $ty {
            const NAME: &'static str = $name;
            fn write(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(bytemuck::bytes_of(self));
            }
            fn read(input: &mut &[u8]) -> Result<Self> {
                take(input, size_of::<$ty>()).map(bytemuck::pod_read_unaligned)
            }
        }
    };
}

pod_component!(Transform, "Transform");
pod_component!(Orbit, "Orbit");
pod_component!(Tint, "Tint");

impl PlayComponent for Label {
    const NAME: &'static str = "Label";
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.0.len() as u32).to_le_bytes());
        out.extend_from_slice(self.0.as_bytes());
    }
    fn read(input: &mut &[u8]) -> Result<Self> {
        let len = u32::from_le_bytes(take(input, 4)?.try_into()?) as usize;
        Ok(Label(String::from_utf8(take(input, len)?.to_vec())?))
    }
}

/// `(entity, component value)` pairs for a batch insert.
pub type JsonRows = Vec<(Entity, Value)>;

/// Type-erased operations for one registered component.
pub struct ComponentKind {
    pub name: &'static str,
    /// The component as canonical JSON, if present.
    pub get_json: fn(&World, Entity) -> Option<Value>,
    /// Inserts or replaces the component on each entity; values are validated by deserializing.
    pub insert_json: fn(&mut World, JsonRows) -> Result<()>,
    /// Removes the component; returns whether it was present.
    pub remove: fn(&mut World, Entity) -> bool,
    /// Appends the canonical bytes if present; returns whether it was present.
    pub write_bytes: fn(&World, Entity, &mut Vec<u8>) -> bool,
    /// Appends a snapshot column (presence bitset, then present values) for `entities`, in order.
    pub encode_column: fn(&World, &[Entity], &mut Vec<u8>),
    /// Inserts a column written by `encode_column` onto `entities`, in the same order.
    pub decode_column: fn(&mut World, &[Entity], &[u8]) -> Result<()>,
}

const fn kind<T: PlayComponent>() -> ComponentKind {
    ComponentKind {
        name: T::NAME,
        get_json: get_json::<T>,
        insert_json: insert_json::<T>,
        remove: remove::<T>,
        write_bytes: write_bytes::<T>,
        encode_column: encode_column::<T>,
        decode_column: decode_column::<T>,
    }
}

/// Every component that crosses the play boundary, in canonical (hash and snapshot) order.
pub static COMPONENTS: [ComponentKind; 4] = [
    kind::<Label>(),
    kind::<Transform>(),
    kind::<Orbit>(),
    kind::<Tint>(),
];

/// Looks up a component by name, with a did-you-mean list on failure.
pub fn component(name: &str) -> Result<&'static ComponentKind> {
    COMPONENTS.iter().find(|k| k.name == name).ok_or_else(|| {
        let known: Vec<_> = COMPONENTS.iter().map(|k| k.name).collect();
        anyhow!("unknown component {name:?}; known: {known:?}")
    })
}

fn get_json<T: PlayComponent>(world: &World, e: Entity) -> Option<Value> {
    world
        .get::<T>(e)
        .map(|c| serde_json::to_value(c).expect("play components are plain data and serialize"))
}

fn insert_json<T: PlayComponent>(world: &mut World, rows: JsonRows) -> Result<()> {
    let typed = rows
        .into_iter()
        .map(|(e, v)| {
            let c: T = serde_json::from_value(v).with_context(|| format!("invalid {}", T::NAME))?;
            Ok((e, c))
        })
        .collect::<Result<Vec<_>>>()?;
    world
        .try_insert_batch(typed)
        .map_err(|e| anyhow!("insert {}: {e}", T::NAME))
}

fn remove<T: PlayComponent>(world: &mut World, e: Entity) -> bool {
    world
        .get_entity_mut(e)
        .ok()
        .and_then(|mut em| em.take::<T>())
        .is_some()
}

fn write_bytes<T: PlayComponent>(world: &World, e: Entity, out: &mut Vec<u8>) -> bool {
    match world.get::<T>(e) {
        Some(c) => {
            c.write(out);
            true
        }
        None => false,
    }
}

fn encode_column<T: PlayComponent>(world: &World, entities: &[Entity], out: &mut Vec<u8>) {
    let bits = out.len();
    out.resize(bits + entities.len().div_ceil(8), 0);
    for (i, &e) in entities.iter().enumerate() {
        if let Some(c) = world.get::<T>(e) {
            out[bits + i / 8] |= 1 << (i % 8);
            c.write(out);
        }
    }
}

fn decode_column<T: PlayComponent>(
    world: &mut World,
    entities: &[Entity],
    bytes: &[u8],
) -> Result<()> {
    let mut input = bytes;
    let bits = take(&mut input, entities.len().div_ceil(8))?;
    let mut rows = Vec::with_capacity(entities.len());
    for (i, &e) in entities.iter().enumerate() {
        if bits[i / 8] & (1 << (i % 8)) != 0 {
            rows.push((e, T::read(&mut input)?));
        }
    }
    if !input.is_empty() {
        bail!("{} column has {} trailing bytes", T::NAME, input.len());
    }
    world
        .try_insert_batch(rows)
        .map_err(|e| anyhow!("insert {}: {e}", T::NAME))
}

/// All registered components of `e` (optionally filtered by name) as canonical JSON.
pub fn entity_json(world: &World, e: Entity, only: Option<&[String]>) -> BTreeMap<String, Value> {
    COMPONENTS
        .iter()
        .filter(|k| only.is_none_or(|names| names.iter().any(|n| n == k.name)))
        .filter_map(|k| (k.get_json)(world, e).map(|v| (k.name.to_owned(), v)))
        .collect()
}

/// Resolves a stable ID to this world's runtime handle.
pub fn entity_of(world: &World, id: SceneId) -> Result<Entity> {
    world
        .resource::<IdIndex>()
        .0
        .get(&id)
        .copied()
        .ok_or_else(|| anyhow!("no entity {}", id.key()))
}

/// The JSON value at `Component.field.path` on `target`, if it exists.
pub fn read_path(world: &World, target: SceneId, path: &str) -> Option<Value> {
    let e = entity_of(world, target).ok()?;
    let (name, field) = split_path(path);
    let whole = (component(name).ok()?.get_json)(world, e)?;
    match field {
        None => Some(whole),
        Some(field) => whole
            .pointer(&format!("/{}", field.replace('.', "/")))
            .cloned(),
    }
}

/// One authoring mutation, scoped to a single scene document (R03 §4.2). `prev` and `snapshot`
/// carry the old state, so every op has a free inverse. The same vocabulary drives live patches
/// in the player and "Keep changes" in the host.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// Sets a whole component (`path: "Orbit"`, adding it if absent) or one field inside it
    /// (`"Orbit.radius"`, `"Transform.position.1"`).
    Set {
        target: SceneId,
        path: String,
        value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prev: Option<Value>,
    },
    /// Removes one component.
    Remove {
        target: SceneId,
        component: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prev: Option<Value>,
    },
    /// Creates an entity. Without `id`, the world allocates the next free one.
    Create {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<SceneId>,
        components: BTreeMap<String, Value>,
    },
    /// Destroys an entity; `snapshot` holds its components for the inverse.
    Delete {
        target: SceneId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot: Option<BTreeMap<String, Value>>,
    },
}

/// Applies `ops` atomically: on the first failure every applied op is undone and the error names
/// the failing index. Returns the inverse ops, already in undo order.
pub fn apply_ops(world: &mut World, ops: &[Op]) -> Result<Vec<Op>> {
    let mut inverse = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        match apply_op(world, op) {
            Ok(inv) => inverse.push(inv),
            Err(err) => {
                for inv in inverse.iter().rev() {
                    apply_op(world, inv).expect("inverse of an applied op always applies");
                }
                return Err(err.context(format!("op {i} failed; nothing applied")));
            }
        }
    }
    inverse.reverse();
    Ok(inverse)
}

fn split_path(path: &str) -> (&str, Option<&str>) {
    match path.split_once('.') {
        Some((component, field)) => (component, Some(field)),
        None => (path, None),
    }
}

fn apply_op(world: &mut World, op: &Op) -> Result<Op> {
    match op {
        Op::Set {
            target,
            path,
            value,
            ..
        } => {
            let e = entity_of(world, *target)?;
            let (name, field) = split_path(path);
            let kind = component(name)?;
            let current = (kind.get_json)(world, e);
            let (new, old) = match field {
                None => (value.clone(), current),
                Some(field) => {
                    let mut whole = current
                        .ok_or_else(|| anyhow!("{} has no {name} component", target.key()))?;
                    let pointer = format!("/{}", field.replace('.', "/"));
                    let slot = whole
                        .pointer_mut(&pointer)
                        .ok_or_else(|| anyhow!("{name} has no field {field:?}"))?;
                    let old = std::mem::replace(slot, value.clone());
                    (whole, Some(old))
                }
            };
            (kind.insert_json)(world, vec![(e, new)]).with_context(|| format!("set {path}"))?;
            Ok(match old {
                Some(old) => Op::Set {
                    target: *target,
                    path: path.clone(),
                    value: old,
                    prev: Some(value.clone()),
                },
                None => Op::Remove {
                    target: *target,
                    component: name.to_owned(),
                    prev: Some(value.clone()),
                },
            })
        }
        Op::Remove {
            target,
            component: name,
            ..
        } => {
            let e = entity_of(world, *target)?;
            let kind = component(name)?;
            let old = (kind.get_json)(world, e)
                .ok_or_else(|| anyhow!("{} has no {name} component", target.key()))?;
            (kind.remove)(world, e);
            Ok(Op::Set {
                target: *target,
                path: name.clone(),
                value: old,
                prev: None,
            })
        }
        Op::Create { id, components } => {
            let id = match id {
                Some(id) => *id,
                None => SceneId(world.resource::<NextSceneId>().0),
            };
            if world.resource::<IdIndex>().0.contains_key(&id) {
                bail!("entity {} already exists", id.key());
            }
            let kinds = components
                .keys()
                .map(|name| component(name))
                .collect::<Result<Vec<_>>>()?;
            let e = world.spawn(id).id();
            for (kind, value) in kinds.iter().zip(components.values()) {
                if let Err(err) = (kind.insert_json)(world, vec![(e, value.clone())]) {
                    world.despawn(e);
                    return Err(err);
                }
            }
            world.resource_mut::<IdIndex>().0.insert(id, e);
            let mut next = world.resource_mut::<NextSceneId>();
            next.0 = next.0.max(id.0 + 1);
            Ok(Op::Delete {
                target: id,
                snapshot: Some(components.clone()),
            })
        }
        Op::Delete { target, .. } => {
            let e = entity_of(world, *target)?;
            let snapshot = entity_json(world, e, None);
            world.despawn(e);
            world.resource_mut::<IdIndex>().0.remove(target);
            Ok(Op::Create {
                id: Some(*target),
                components: snapshot,
            })
        }
    }
}
