//! Read-only scene identity/hierarchy queries over the active exclusive ECS scope.
use super::ctx::{bytes, ctx};
use crate::abi::{SCENE_CHILDREN, SCENE_FIND, SCENE_PARENT, SCENE_SIBLINGS};
use ara_sim::{Identity, SceneMetadata};
use bevy_ecs::{entity::Entity, prelude::ChildOf, resource::IsResource, world::World};
use core::ffi::c_void;

fn parent(world: &World, entity: Entity) -> Option<Entity> {
    world.get::<ChildOf>(entity).map(ChildOf::parent)
}

fn named_path(world: &World, mut entity: Entity, names: &[&str]) -> bool {
    for (index, name) in names.iter().enumerate().rev() {
        if world
            .get::<SceneMetadata>(entity)
            .is_none_or(|m| m.name != *name)
        {
            return false;
        }
        if index == 0 {
            return parent(world, entity).is_none();
        }
        match parent(world, entity) {
            Some(p) => entity = p,
            None => return false,
        }
    }
    false
}

pub(super) unsafe extern "C" fn scene_entities(
    context: *mut c_void,
    operation: u32,
    bits: u64,
    name: *const u8,
    len: u32,
    output: *mut u64,
    capacity: u32,
) -> u32 {
    // SAFETY: the ABI caller supplies its host handle and a borrowed UTF-8 span.
    let (context, bytes) = unsafe { (ctx(context), bytes(name, len)) };
    let Some(scope) = context.current() else {
        return 0;
    };
    // SAFETY: only the owning thread can obtain the active exclusive scope. Queries
    // borrow the world only for this callback and never retain ECS references.
    let world = unsafe { scope.world.world() };
    let entity = Entity::try_from_bits(bits).filter(|e| world.get_entity(*e).is_ok());
    let selector = std::str::from_utf8(bytes)
        .unwrap_or("")
        .trim_start_matches("name:")
        .trim_matches('/');
    let names: Vec<_> = selector.split('/').collect();
    let owner = entity.and_then(|e| parent(world, e));
    let mut matches: Vec<_> = world
        .archetypes()
        .iter()
        .flat_map(|a| a.entities().iter().map(|e| e.id()))
        .filter(|candidate| world.get::<IsResource>(*candidate).is_none())
        .filter(|candidate| match operation {
            SCENE_FIND => {
                world
                    .get::<Identity>(*candidate)
                    .is_some_and(|id| id.0 == selector)
                    || if selector.contains('/') {
                        named_path(world, *candidate, &names)
                    } else {
                        !selector.is_empty()
                            && world
                                .get::<SceneMetadata>(*candidate)
                                .is_some_and(|m| m.name == selector)
                    }
            }
            SCENE_PARENT => entity.is_some_and(|e| parent(world, e) == Some(*candidate)),
            SCENE_CHILDREN => entity.is_some_and(|e| parent(world, *candidate) == Some(e)),
            SCENE_SIBLINGS => {
                entity.is_some_and(|e| *candidate != e && parent(world, *candidate) == owner)
            }
            _ => false,
        })
        .collect();
    if operation == SCENE_FIND
        && matches.iter().any(|entity| {
            world
                .get::<Identity>(*entity)
                .is_some_and(|id| id.0 == selector)
        })
    {
        // Stable identities take precedence over a display name that resembles an id.
        matches.retain(|entity| {
            world
                .get::<Identity>(*entity)
                .is_some_and(|id| id.0 == selector)
        });
    }
    matches.sort_unstable_by(|a, b| {
        let key = |e| {
            (
                world
                    .get::<SceneMetadata>(e)
                    .map_or("", |m| m.order.as_str()),
                world.get::<Identity>(e).map_or("", |i| i.0.as_str()),
                e.to_bits(),
            )
        };
        key(*a).cmp(&key(*b))
    });
    if matches.len() <= capacity as usize && !output.is_null() {
        for (n, entity) in matches.iter().enumerate() {
            // SAFETY: capacity writable handles are borrowed by the caller; n is in range.
            unsafe {
                output.add(n).write(entity.to_bits());
            }
        }
    }
    matches.len() as u32
}
