//! Skeletal animation in the fixed tick. Each entity with an `Animator` and a skinned
//! `MeshRenderer.model` owns an [`AnimatorSlot`]: an ara-anim animator running a
//! compiled [`Program`] (idle state + speed-driven 1D blend tree). Animator state is
//! captured into snapshots by [`AnimationProvider`]; poses become skinning palettes
//! only in presentation.
use crate::{
    Result, RuntimeError,
    assets::{Models, asset_id},
    components::{Animator, MeshRenderer},
    model::Model,
    skin::{Skin, TICKS_PER_SECOND},
};
use ara_anim::{
    CompressionSettings, Controller, CookedClip, RootMotion,
    glam::Mat4,
    graph::{
        BlendSample, Comparison, Condition, ControllerData, Node, NodeKind, Parameter, ParameterId,
        ParameterKind, ParameterValue, State, Transition,
    },
};
use ara_nav::NavAgentState;
use ara_physics::Velocity;
use ara_schema::AssetRef;
use ara_sim::{Identity, SimError, StateProvider};
use bevy_ecs::prelude::*;
use serde_json::Value;
use std::{any::TypeId, collections::BTreeMap, sync::Arc};

/// A compiled animation program shared by every animator with equal settings.
pub struct Program {
    /// Model generation providing the skin and clips.
    pub model: Arc<Model>,
    /// Skin index within the model.
    pub skin: usize,
    /// Immutable ara-anim program.
    pub controller: Controller,
    /// The `speed` float parameter.
    pub speed: ParameterId,
}

impl Program {
    /// The skin this program animates.
    pub fn skin(&self) -> &Skin {
        &self.model.skins[self.skin]
    }
}

/// Decoded models and compiled programs; immutable between tick boundaries.
#[derive(Resource, Default)]
pub struct Library {
    /// Live model generations by asset ID.
    pub models: Models,
    programs: BTreeMap<String, Arc<Program>>,
}

impl Library {
    /// The program for `animator` on `model`, compiled once per generation and settings.
    pub fn program(
        &mut self,
        model: &Arc<Model>,
        animator: &Animator,
        hz: u32,
    ) -> Result<Arc<Program>> {
        let key = format!(
            "{}:{}:{hz}:{}",
            model.asset,
            model.digest,
            serde_json::to_string(animator)?
        );
        if let Some(program) = self.programs.get(&key) {
            return Ok(Arc::clone(program));
        }
        let program = Arc::new(compile(model, animator, hz)?);
        self.programs.insert(key, Arc::clone(&program));
        Ok(program)
    }
    /// Drops programs of replaced model generations.
    pub fn retain_live(&mut self) {
        let models = &self.models;
        self.programs.retain(|_, p| {
            models
                .get(&p.model.asset)
                .is_some_and(|m| m.digest == p.model.digest)
        });
    }
}

/// Per-entity animation runtime (not reflected; captured by [`AnimationProvider`]).
#[derive(Component)]
pub struct AnimatorSlot {
    /// Program this animator runs.
    pub program: Arc<Program>,
    /// ara-anim playback state and derived buffers.
    pub animator: ara_anim::Animator,
    /// Root displacement extracted by the latest tick.
    pub root: RootMotion,
    /// First runtime failure; the slot stops advancing until restored.
    pub fault: Option<String>,
}

impl AnimatorSlot {
    fn new(program: Arc<Program>) -> Result<Self> {
        let animator = ara_anim::Animator::new(&program.controller, &program.skin().skeleton)
            .map_err(|e| RuntimeError(e.0))?;
        Ok(Self {
            program,
            animator,
            root: RootMotion::default(),
            fault: None,
        })
    }

    /// Skinning palette in the mesh's skin joint order (model space), reusing buffers.
    pub fn palette(&self, model: &mut Vec<Mat4>, out: &mut Vec<Mat4>) -> Result<()> {
        let skin = self.program.skin();
        let joints = skin.skeleton.len();
        model.resize(joints, Mat4::IDENTITY);
        out.resize(joints, Mat4::IDENTITY);
        ara_anim::pose::model_matrices(&skin.skeleton, self.animator.pose(), model)
            .map_err(|e| RuntimeError(e.0))?;
        ara_anim::pose::skinning_palette(&skin.skeleton, model, out)
            .map_err(|e| RuntimeError(e.0))?;
        // Reorder skeleton order → skin order, reusing `model` as scratch.
        model.copy_from_slice(out);
        for (skin_index, &anim_index) in skin.joint_map.iter().enumerate() {
            out[skin_index] = model[anim_index];
        }
        Ok(())
    }
}

/// Builds the ara-anim controller source for an `Animator`.
pub fn controller_data(
    model: &Model,
    animator: &Animator,
    hz: u32,
) -> Result<(ControllerData, Vec<String>)> {
    if !TICKS_PER_SECOND.is_multiple_of(hz) {
        return Err(RuntimeError(format!(
            "tick rate {hz} Hz must divide the {TICKS_PER_SECOND} Hz clip timebase"
        )));
    }
    let clip_ref = |i: usize| AssetRef(format!("@clips/{i}#{}", model.asset));
    let mut names = vec![animator.idle.clone()];
    names.extend(animator.locomotion.iter().map(|c| c.clip.clone()));
    let mut data = ControllerData {
        parameters: BTreeMap::from([(
            "speed".to_owned(),
            Parameter {
                kind: ParameterKind::Float,
                ..Default::default()
            },
        )]),
        initial: "idle".into(),
        fixed_step: TICKS_PER_SECOND / hz,
        extract_root_motion: true,
        ..Default::default()
    };
    data.nodes.insert(
        "idle".into(),
        Node {
            kind: NodeKind::Clip,
            clip: Some(clip_ref(0)),
            ..Default::default()
        },
    );
    data.states.insert(
        "idle".into(),
        State {
            node: "idle".into(),
            ..Default::default()
        },
    );
    if !animator.locomotion.is_empty() {
        let mut samples = Vec::new();
        for (i, clip) in animator.locomotion.iter().enumerate() {
            let node = format!("move{i}");
            data.nodes.insert(
                node.clone(),
                Node {
                    kind: NodeKind::Clip,
                    clip: Some(clip_ref(i + 1)),
                    ..Default::default()
                },
            );
            samples.push(BlendSample {
                node,
                x: clip.speed,
                y: 0.0,
            });
        }
        data.nodes.insert(
            "move".into(),
            Node {
                kind: NodeKind::Blend1d,
                parameter_x: "speed".into(),
                samples,
                ..Default::default()
            },
        );
        let skin = model
            .skins
            .first()
            .ok_or_else(|| RuntimeError(format!("{} has no skin", model.asset)))?;
        // The blend tree samples one normalized phase; its cycle is the mean length of
        // the moving clips so neither end of the blend is stretched to the extreme.
        let durations = animator
            .locomotion
            .iter()
            .map(|c| clip(skin, &c.clip).map(|d| u64::from(d.duration)))
            .collect::<Result<Vec<_>>>()?;
        let cycle = durations.iter().sum::<u64>() / durations.len() as u64;
        data.states.insert(
            "move".into(),
            State {
                node: "move".into(),
                duration: u32::try_from(cycle).unwrap_or(u32::MAX),
                ..Default::default()
            },
        );
        let fade = (animator.fade * TICKS_PER_SECOND as f32).round() as u32;
        let speed = |comparison, value: f32| Condition {
            parameter: "speed".into(),
            comparison,
            float_value: value,
            ..Default::default()
        };
        data.transitions = vec![
            Transition {
                id: "start".into(),
                from: "idle".into(),
                to: "move".into(),
                duration: fade,
                conditions: vec![speed(Comparison::Greater, animator.move_threshold)],
                ..Default::default()
            },
            Transition {
                id: "stop".into(),
                from: "move".into(),
                to: "idle".into(),
                duration: fade,
                conditions: vec![speed(Comparison::Less, animator.move_threshold * 0.5)],
                ..Default::default()
            },
        ];
    }
    Ok((data, names))
}

fn clip<'a>(skin: &'a Skin, name: &str) -> Result<&'a ara_anim::data::ClipData> {
    skin.clips
        .get(name)
        .ok_or_else(|| RuntimeError(format!("model has no animation named `{name}`")))
}

fn compile(model: &Arc<Model>, animator: &Animator, hz: u32) -> Result<Program> {
    let skin = model
        .skins
        .first()
        .ok_or_else(|| RuntimeError(format!("{} has no skin", model.asset)))?;
    let (data, names) = controller_data(model, animator, hz)?;
    let mut bindings = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let (cooked, _) = CookedClip::cook(
            clip(skin, name)?,
            &skin.skeleton,
            CompressionSettings::default(),
        )
        .map_err(|e| RuntimeError(format!("clip `{name}`: {}", e.0)))?;
        bindings.push((AssetRef(format!("@clips/{i}#{}", model.asset)), cooked));
    }
    let controller =
        Controller::compile(&data, &skin.skeleton, bindings).map_err(|e| RuntimeError(e.0))?;
    let speed = controller
        .parameter("speed")
        .ok_or_else(|| RuntimeError("compiled program lost its speed parameter".into()))?;
    Ok(Program {
        model: Arc::clone(model),
        skin: 0,
        controller,
        speed,
    })
}

/// Creates slots for animated entities that lack one (setup, restore, reload).
/// Entities whose model is not loaded or has no skin are skipped.
pub fn ensure_slots(world: &mut World, hz: u32) -> Result<()> {
    let mut wanted = Vec::new();
    let mut query =
        world.query_filtered::<(Entity, &Animator, &MeshRenderer), Without<AnimatorSlot>>();
    for (entity, animator, renderer) in query.iter(world) {
        if let Some(model) = &renderer.model {
            wanted.push((entity, animator.clone(), asset_id(model)?.to_owned()));
        }
    }
    for (entity, animator, asset) in wanted {
        let program = {
            let mut library = world.resource_mut::<Library>();
            let Some(model) = library.models.get(&asset).cloned() else {
                continue;
            };
            if model.skins.is_empty() {
                continue;
            }
            library.program(&model, &animator, hz)?
        };
        let slot = AnimatorSlot::new(program)?;
        world.entity_mut(entity).insert(slot);
    }
    Ok(())
}

/// Fixed-tick animation: sets `speed` from the entity's ground velocity (navigation
/// state when present, else physics velocity) and advances one tick, keeping the
/// extracted root displacement. Entities are independent, so order is irrelevant.
pub fn animate(mut query: Query<(&mut AnimatorSlot, Option<&Velocity>, Option<&NavAgentState>)>) {
    for (mut slot, velocity, nav) in &mut query {
        if slot.fault.is_some() {
            continue;
        }
        let v = nav
            .map(|n| n.velocity)
            .or_else(|| velocity.map(|v| v.linear))
            .unwrap_or_default();
        let speed = (v[0] * v[0] + v[2] * v[2]).sqrt();
        let slot = &mut *slot;
        let result = slot
            .animator
            .set(slot.program.speed, ParameterValue::Float(speed))
            .and_then(|()| {
                slot.animator.tick(
                    &slot.program.controller,
                    &slot.program.skin().skeleton,
                    |_| {},
                )
            });
        match result {
            Ok(root) => slot.root = root,
            Err(error) => slot.fault = Some(error.0),
        }
    }
}

/// Captures animator state into snapshots and restores it, recreating missing slots.
pub struct AnimationProvider {
    /// Simulation tick rate, needed to compile programs on restore.
    pub hz: u32,
}

impl StateProvider for AnimationProvider {
    fn name(&self) -> &'static str {
        "animation"
    }
    fn owned_types(&self) -> Vec<TypeId> {
        vec![TypeId::of::<AnimatorSlot>(), TypeId::of::<Library>()]
    }
    fn capture(&self, world: &World) -> std::result::Result<Value, SimError> {
        let mut rows = Vec::new();
        for entity in world.iter_entities() {
            if let (Some(id), Some(slot)) = (entity.get::<Identity>(), entity.get::<AnimatorSlot>())
            {
                rows.push((
                    id.0.clone(),
                    serde_json::to_value(slot.animator.snapshot())?,
                ));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(serde_json::to_value(rows)?)
    }
    fn restore(&self, world: &mut World, state: &Value) -> std::result::Result<(), SimError> {
        ensure_slots(world, self.hz).map_err(|e| SimError(e.0))?;
        let rows: BTreeMap<String, ara_anim::Snapshot> =
            serde_json::from_value::<Vec<(String, ara_anim::Snapshot)>>(state.clone())?
                .into_iter()
                .collect();
        let mut query = world.query::<(&Identity, &mut AnimatorSlot)>();
        for (id, mut slot) in query.iter_mut(world) {
            let slot = &mut *slot;
            match rows.get(&id.0) {
                Some(snapshot) => slot
                    .animator
                    .restore(
                        &slot.program.controller,
                        &slot.program.skin().skeleton,
                        snapshot,
                    )
                    .map_err(|e| SimError(format!("animator {}: {}", id.0, e.0)))?,
                None => {
                    slot.animator = ara_anim::Animator::new(
                        &slot.program.controller,
                        &slot.program.skin().skeleton,
                    )
                    .map_err(|e| SimError(e.0))?;
                }
            }
            slot.root = RootMotion::default();
            slot.fault = None;
        }
        Ok(())
    }
}
