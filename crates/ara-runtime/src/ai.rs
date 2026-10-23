//! Gameplay AI in play (ara-ai on ara-nav v2): `Brain` entities run compiled
//! behaviour programs (graph documents of domain `behavior`), perceive `Stimulus`
//! entities and write intents; navigation follows `MoveIntent`, animation reads
//! `LocomotionIntent`.
//!
//! Per tick (see the order in `lib.rs`): `ai.poses` (perception poses of non-agents) →
//! `ai.adopt` (streamed brains) → `ai.sync` → `ai.perceive` → `ai.think` → `ai.bridge`
//! → `navigation` → `ai.locomotion` → `ai.report`. Every brain's runtime state
//! (`BrainState`, `Perception`, intents) is captured by [`AiProvider`]; programs are
//! immutable data rebuilt from the documents.
use crate::{Result, RuntimeError, math::stable_id, sync::WorldPose};
use ara_ai::{
    ActionIntent, AiConfig, AiWorld, Brain, BrainState, LocomotionIntent, LookIntent, MoveIntent,
};
use ara_doc::Workspace;
use ara_nav::{NavAgent, NavAgentState, NavPosition};
use ara_perception::{
    NoiseEvents, Perceiver, Perception, PerceptionConfig, PerceptionPose, Stimulus,
};
use ara_sim::{Identity, SimError, StateProvider};
use bevy_ecs::prelude::*;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{any::TypeId, collections::BTreeMap};

/// Installs the AI resources, compiles every behaviour graph in `documents` and gives
/// brains and stimuli their runtime state. Call after navigation installed (brains
/// share their agent's stable ID). Behaviour errors fail the build with their
/// diagnostics.
pub fn install(world: &mut World, documents: &Workspace, fixed_dt: f32) -> Result<()> {
    ara_ai::register_sim(
        world,
        &mut Schedule::default(),
        AiConfig {
            fixed_dt,
            ..Default::default()
        },
    );
    for (path, doc) in documents.docs() {
        if doc.kind != "graph" {
            continue;
        }
        let Ok(graph) = ara_graph::Graph::from_document(doc) else {
            continue;
        };
        if graph.header.domain != "behavior" {
            continue;
        }
        let behavior = ara_ai::projection::from_graph(&graph)
            .map_err(|d| RuntimeError(format!("{path}: {}", d.message)))?;
        let program = ara_ai::compile(&behavior).map_err(|d| {
            let messages: Vec<String> = d.iter().map(|d| d.message.clone()).collect();
            RuntimeError(format!("{path}: {}", messages.join("; ")))
        })?;
        world
            .resource_mut::<AiWorld>()
            .insert_program(doc.id.to_string(), program);
    }
    adopt(world);
    Ok(())
}

/// The feet position and facing of an entity's world pose.
fn pose_of(pose: &WorldPose) -> ([f32; 3], [f32; 3]) {
    let forward = pose.0.rotation * Vec3::Z;
    (pose.0.translation.to_array(), [forward.x, 0.0, forward.z])
}

/// Gives brains (with a navigation agent) and stimuli/perceivers their runtime
/// components when they lack them: at build and for streamed-in entities.
pub fn adopt(world: &mut World) {
    let mut brains = Vec::new();
    let mut query = world.query_filtered::<(
        Entity,
        &Identity,
        &Brain,
        Option<&NavPosition>,
        Option<&WorldPose>,
    ), (With<NavAgent>, Without<BrainState>)>();
    for (entity, id, brain, position, pose) in query.iter(world) {
        let at = position
            .map(|p| p.0)
            .or_else(|| pose.map(|p| pose_of(p).0))
            .unwrap_or_default();
        let key = brain.program_key().unwrap_or_default().to_owned();
        brains.push((entity, stable_id(&id.0), key, at));
    }
    for (entity, id, key, at) in brains {
        let state = world.get::<NavAgentState>(entity).cloned();
        let mut entity = world.entity_mut(entity);
        entity.insert(ara_ai::bundle(id, &key, at));
        // Navigation already owns this agent's state (same stable ID).
        if let Some(state) = state {
            entity.insert(state);
        }
    }
    let mut targets = Vec::new();
    let mut query = world.query_filtered::<(Entity, &Identity, Option<&WorldPose>), (
        Or<(With<Stimulus>, With<Perceiver>)>,
        Without<PerceptionPose>,
    )>();
    for (entity, id, pose) in query.iter(world) {
        let (position, forward) = pose.map_or(([0.0; 3], [0.0, 0.0, 1.0]), pose_of);
        targets.push((
            entity,
            PerceptionPose {
                id: stable_id(&id.0),
                position,
                forward,
            },
        ));
    }
    for (entity, pose) in targets {
        world.entity_mut(entity).insert(pose);
    }
}

/// Perception poses of stimuli and perceivers that are not navigation agents (players,
/// props) follow their world pose; agents are synced from navigation by `ai.sync`.
pub fn poses(mut query: Query<(&mut PerceptionPose, &WorldPose), Without<NavPosition>>) {
    for (mut perceived, pose) in &mut query {
        let (position, forward) = pose_of(pose);
        if perceived.position != position || perceived.forward != forward {
            perceived.position = position;
            perceived.forward = forward;
        }
    }
}

/// One entity's AI runtime state in a snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Row {
    brain: Option<BrainState>,
    pose: Option<PerceptionPose>,
    perception: Option<Perception>,
    moves: Option<MoveIntent>,
    look: Option<LookIntent>,
    action: Option<ActionIntent>,
    locomotion: Option<LocomotionIntent>,
}

/// Captures brain, perception and intent state by entity identity and restores it.
/// `AiWorld` (programs, trace) and the noise queue (empty at every boundary) are
/// derived infrastructure.
pub struct AiProvider;

impl StateProvider for AiProvider {
    fn name(&self) -> &'static str {
        "ai"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![
            TypeId::of::<AiWorld>(),
            TypeId::of::<PerceptionConfig>(),
            TypeId::of::<NoiseEvents>(),
            TypeId::of::<BrainState>(),
            TypeId::of::<PerceptionPose>(),
            TypeId::of::<Perception>(),
            TypeId::of::<MoveIntent>(),
            TypeId::of::<LookIntent>(),
            TypeId::of::<ActionIntent>(),
            TypeId::of::<LocomotionIntent>(),
        ]
    }
    fn capture(&self, world: &World) -> std::result::Result<Value, SimError> {
        let mut rows = BTreeMap::new();
        for entity in world.iter_entities() {
            let Some(id) = entity.get::<Identity>() else {
                continue;
            };
            let row = Row {
                brain: entity.get::<BrainState>().cloned(),
                pose: entity.get::<PerceptionPose>().copied(),
                perception: entity.get::<Perception>().cloned(),
                moves: entity.get::<MoveIntent>().copied(),
                look: entity.get::<LookIntent>().copied(),
                action: entity.get::<ActionIntent>().cloned(),
                locomotion: entity.get::<LocomotionIntent>().copied(),
            };
            if row.brain.is_some() || row.pose.is_some() {
                rows.insert(id.0.clone(), row);
            }
        }
        Ok(serde_json::to_value(rows)?)
    }
    fn restore(&self, world: &mut World, state: &Value) -> std::result::Result<(), SimError> {
        let mut rows: BTreeMap<String, Row> = serde_json::from_value(state.clone())?;
        let mut entities = Vec::new();
        let mut query = world.query::<(Entity, &Identity)>();
        for (entity, id) in query.iter(world) {
            entities.push((entity, id.0.clone()));
        }
        for (entity, id) in entities {
            let row = rows.remove(&id).unwrap_or_default();
            let mut entity = world.entity_mut(entity);
            entity.remove::<(
                BrainState,
                PerceptionPose,
                Perception,
                MoveIntent,
                LookIntent,
                ActionIntent,
                LocomotionIntent,
            )>();
            if let Some(v) = row.brain {
                entity.insert(v);
            }
            if let Some(v) = row.pose {
                entity.insert(v);
            }
            if let Some(v) = row.perception {
                entity.insert(v);
            }
            if let Some(v) = row.moves {
                entity.insert(v);
            }
            if let Some(v) = row.look {
                entity.insert(v);
            }
            if let Some(v) = row.action {
                entity.insert(v);
            }
            if let Some(v) = row.locomotion {
                entity.insert(v);
            }
        }
        if let Some(id) = rows.keys().next() {
            return Err(SimError(format!("snapshot has AI state for unknown {id}")));
        }
        Ok(())
    }
}
