//! Skeletal animation in the fixed tick. Each entity with an `Animator` and a skinned
//! `MeshRenderer.model` owns an [`AnimatorSlot`]: an ara-anim animator running a
//! compiled [`Program`]: the `.animation` controller of `Animator.controller`
//! ([`crate::controller`]: state machines, blend trees, motion matching), else the
//! built-in idle state + speed-driven 1D blend tree. A float `speed` parameter, when
//! present, follows ground speed. Animator and motion-matching predictor state are
//! captured into snapshots by [`AnimationProvider`]; poses become skinning palettes
//! only in presentation.
use crate::{
    Result, RuntimeError,
    assets::{Models, asset_id},
    components::{Animator, MeshRenderer},
    controller::{Documents, MatchProgram, MatchSnapshot, MatchState},
    model::Model,
    skin::{Skin, TICKS_PER_SECOND},
};
use ara_ai::LocomotionIntent;
use ara_anim::{
    CompressionSettings, Controller, CookedClip, RootMotion,
    glam::{Mat4, Vec3},
    graph::{
        BlendSample, Comparison, Condition, ControllerData, Node, NodeKind, Parameter, ParameterId,
        ParameterKind, ParameterValue, State, Transition,
    },
};
use ara_components::Transform;
use ara_nav::NavAgentState;
use ara_physics::Velocity;
use ara_schema::AssetRef;
use ara_sim::{Identity, SimContext, SimError, StateProvider};
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
    /// The `speed` float parameter, when the controller has one.
    pub speed: Option<ParameterId>,
    /// Motion matching, when the controller has a motion-matching node.
    pub matching: Option<MatchProgram>,
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
    /// Native graph/instance overrides resolved outside frame extraction.
    pub materials: crate::material::Materials,
    /// Post-process volumes' colour lookup tables, decoded outside frame extraction.
    pub luts: crate::lut::Luts,
    /// The project's `.animation` records (controllers, clips, motion databases).
    pub documents: Documents,
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
        let program = Arc::new(match &animator.controller {
            Some(reference) => {
                let compiled = crate::controller::compile(model, &self.documents, reference, hz)?;
                Program {
                    model: Arc::clone(model),
                    skin: 0,
                    speed: compiled.controller.parameter("speed"),
                    controller: compiled.controller,
                    matching: compiled.matching,
                }
            }
            None => compile(model, animator, hz)?,
        });
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
    /// Motion-matching predictor and feature scratch.
    pub matching: Option<MatchState>,
}

impl AnimatorSlot {
    fn new(program: Arc<Program>, transform: Option<&Transform>) -> Result<Self> {
        let animator = ara_anim::Animator::new(&program.controller, &program.skin().skeleton)
            .map_err(|e| RuntimeError(e.0))?;
        let (position, yaw) = transform.map_or((Vec3::ZERO, 0.0), |t| {
            (Vec3::from_array(t.position), t.rotation[1].to_radians())
        });
        let matching = match &program.matching {
            Some(m) => Some(MatchState::new(
                m,
                program.skin(),
                &animator,
                position,
                yaw,
            )?),
            None => None,
        };
        Ok(Self {
            program,
            animator,
            root: RootMotion::default(),
            fault: None,
            matching,
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
    Ok(Program {
        model: Arc::clone(model),
        skin: 0,
        speed: controller.parameter("speed"),
        controller,
        matching: None,
    })
}

/// Creates slots for animated entities that lack one (setup, restore, reload).
/// Entities whose model is not loaded or has no skin are skipped.
pub fn ensure_slots(world: &mut World, hz: u32) -> Result<()> {
    let mut wanted = Vec::new();
    let mut query = world.query_filtered::<
        (Entity, &Animator, &MeshRenderer, Option<&Transform>),
        Without<AnimatorSlot>,
    >();
    for (entity, animator, renderer, transform) in query.iter(world) {
        if let Some(model) = &renderer.model {
            wanted.push((
                entity,
                animator.clone(),
                asset_id(model)?.to_owned(),
                transform.copied(),
            ));
        }
    }
    for (entity, animator, asset, transform) in wanted {
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
        let slot = AnimatorSlot::new(program, transform.as_ref())?;
        world.entity_mut(entity).insert(slot);
    }
    Ok(())
}

/// Gives streamed-in animators (spawned after build) their slot at the boundary that
/// spawned them; already-slotted entities are untouched. A program that fails to
/// compile leaves the entity unanimated and is reported through its next slot fault.
pub fn adopt_slots(world: &mut World) {
    let hz = (1.0 / world.resource::<SimContext>().dt()).round() as u32;
    let _ = ensure_slots(world, hz);
}

type Animated<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut AnimatorSlot,
        Option<&'static Velocity>,
        Option<&'static NavAgentState>,
        Option<&'static LocomotionIntent>,
    ),
>;

/// Fixed-tick animation: sets `speed` from the entity's ground velocity (an NPC's
/// `LocomotionIntent`, else navigation state, else physics velocity) and advances one
/// tick, keeping the extracted root displacement. Motion matching steers toward the
/// intent's desired velocity. Entities are independent, so order is irrelevant.
pub fn animate(context: Res<SimContext>, mut query: Animated) {
    let dt = context.dt() as f32;
    for (mut slot, velocity, nav, intent) in &mut query {
        if slot.fault.is_some() {
            continue;
        }
        let v = intent
            .map(|i| i.velocity)
            .or_else(|| nav.map(|n| n.velocity))
            .or_else(|| velocity.map(|v| v.linear))
            .unwrap_or_default();
        let desired = intent.map_or(v, |i| i.desired);
        let speed = (v[0] * v[0] + v[2] * v[2]).sqrt();
        let slot = &mut *slot;
        let program = &*slot.program;
        let result = program
            .speed
            .map_or(Ok(()), |id| {
                slot.animator.set(id, ParameterValue::Float(speed))
            })
            .and_then(|()| match (&mut slot.matching, &program.matching) {
                (Some(state), Some(matching)) => state.query(
                    matching,
                    program.skin(),
                    &mut slot.animator,
                    Vec3::from_array(desired),
                    dt,
                ),
                _ => Ok(()),
            })
            .and_then(|()| {
                slot.animator
                    .tick(&program.controller, &program.skin().skeleton, |_| {})
            })
            .and_then(|root| match &mut slot.matching {
                Some(state) => state.observe(slot.animator.pose()).map(|()| root),
                None => Ok(root),
            });
        match result {
            Ok(root) => slot.root = root,
            Err(error) => slot.fault = Some(error.0),
        }
    }
}

/// Name of the read-only pseudo-component that reports a live animator to tools
/// (`play.read e_fox.AnimatorState`, the editor's live state machine). It is derived
/// on each read: never authored, snapshotted or replicated.
pub const STATUS: &str = "AnimatorState";

/// The live animator of `entity` as tools read it ([`STATUS`]): `ara_anim::Status`
/// plus the slot's fault. `None` for other names and entities without an animator.
pub fn read_status(world: &World, entity: Entity, name: &str) -> Option<Value> {
    if name != STATUS {
        return None;
    }
    let slot = world.get::<AnimatorSlot>(entity)?;
    let mut value = serde_json::to_value(slot.animator.status(&slot.program.controller)).ok()?;
    value["fault"] = serde_json::json!(slot.fault);
    Some(value)
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
                    serde_json::to_value(slot.matching.as_ref().map(MatchState::snapshot))?,
                ));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(serde_json::to_value(rows)?)
    }
    fn restore(&self, world: &mut World, state: &Value) -> std::result::Result<(), SimError> {
        ensure_slots(world, self.hz).map_err(|e| SimError(e.0))?;
        type Row = (String, ara_anim::Snapshot, Option<MatchSnapshot>);
        let rows: BTreeMap<String, (ara_anim::Snapshot, Option<MatchSnapshot>)> =
            serde_json::from_value::<Vec<Row>>(state.clone())?
                .into_iter()
                .map(|(id, snapshot, predictor)| (id, (snapshot, predictor)))
                .collect();
        let mut query = world.query::<(&Identity, &mut AnimatorSlot)>();
        for (id, mut slot) in query.iter_mut(world) {
            let slot = &mut *slot;
            match rows.get(&id.0) {
                Some((snapshot, predictor)) => {
                    slot.animator
                        .restore(
                            &slot.program.controller,
                            &slot.program.skin().skeleton,
                            snapshot,
                        )
                        .map_err(|e| SimError(format!("animator {}: {}", id.0, e.0)))?;
                    if let (Some(state), Some(saved)) = (&mut slot.matching, predictor) {
                        state
                            .restore(saved)
                            .map_err(|e| SimError(format!("animator {}: {}", id.0, e.0)))?;
                    }
                }
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
