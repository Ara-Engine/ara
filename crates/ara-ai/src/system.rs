//! ECS integration: programs, sight policy, the per-tick systems and their order.
//!
//! Order each fixed tick: `sync` (poses from navigation) -> `perceive` -> `think`
//! -> `bridge` (intents to navigation) -> `ara_nav::tick` -> `locomotion` ->
//! `report` (diagnostics for `ara why`). [`register_sim`] installs exactly that.
use crate::components::*;
use crate::eval::Env;
use crate::program::{Program, Value};
use crate::think::{Out, think as think_brain};
use crate::trace::{BrainView, TraceEvent};
use crate::{NavLineOfSight, ast::Expr, codes, dsl};
use ara_diag::Diagnostic;
use ara_nav::{NavAgent, NavAgentState, NavPosition, NavWorld, NavigationTick};
use ara_perception::{
    LineOfSight, NoiseEvents, OpenSpace, Perception, PerceptionConfig, PerceptionPose,
};
use bevy_ecs::{
    change_detection::Mut,
    prelude::{Entity, Resource, World},
    schedule::{IntoScheduleConfigs, Schedule},
};
use rayon::prelude::*;
use std::{collections::BTreeMap, sync::Arc};

/// Fixed-tick AI settings.
#[derive(Debug, Clone, Copy)]
pub struct AiConfig {
    /// Seconds per fixed tick (same as navigation and perception).
    pub fixed_dt: f32,
    /// A leaf state older than this with unfinished work is reported as stuck.
    pub stuck_seconds: f32,
    /// Period of status diagnostics for troubled brains.
    pub report_seconds: f32,
}
impl Default for AiConfig {
    fn default() -> Self {
        Self {
            fixed_dt: 1.0 / 60.0,
            stuck_seconds: 20.0,
            report_seconds: 5.0,
        }
    }
}

/// Line of sight for perception and `visible()`.
pub enum Sight {
    /// Boundary walls of the navmesh published for this profile.
    Navmesh(u32),
    /// No occluders.
    Open,
    /// Custom provider (physics rays via `ara_perception::PhysicsLineOfSight`
    /// need a borrowed world; wrap them in a host system instead).
    Custom(Box<dyn LineOfSight + Send + Sync>),
}

/// Compiled programs, sight policy and the latest debugger trace.
#[derive(Resource)]
pub struct AiWorld {
    pub(crate) config: AiConfig,
    programs: BTreeMap<String, Arc<Program>>,
    sight: Sight,
    trace: Vec<TraceEvent>,
    errors: Vec<(u64, Diagnostic)>,
    poses: Vec<(u64, [f32; 3])>,
}

impl AiWorld {
    /// Empty AI world with navmesh sight for profile 0.
    pub fn new(config: AiConfig) -> Self {
        Self {
            config,
            programs: BTreeMap::new(),
            sight: Sight::Navmesh(0),
            trace: Vec::new(),
            errors: Vec::new(),
            poses: Vec::new(),
        }
    }
    /// Publishes a compiled program under `key` (graph asset ID); brains running
    /// an older hash restart on their next tick (hot reload at a tick boundary).
    pub fn insert_program(&mut self, key: impl Into<String>, program: Program) {
        self.programs.insert(key.into(), Arc::new(program));
    }
    /// Parses and compiles DSL text, then publishes it under `key`.
    pub fn load(&mut self, key: &str, source: &str) -> Result<(), Vec<Diagnostic>> {
        let behavior = dsl::parse(source, key).map_err(|d| vec![d])?;
        let program = crate::compile(&behavior)?;
        self.insert_program(key, program);
        Ok(())
    }
    /// A published program.
    pub fn program(&self, key: &str) -> Option<&Arc<Program>> {
        self.programs.get(key)
    }
    /// Sets the line-of-sight policy.
    pub fn set_sight(&mut self, sight: Sight) {
        self.sight = sight;
    }
    /// Debugger events of the latest tick, ordered by brain ID.
    pub fn trace(&self) -> &[TraceEvent] {
        &self.trace
    }
    /// Problems of the latest tick (missing programs, bad overrides).
    pub fn errors(&self) -> &[(u64, Diagnostic)] {
        &self.errors
    }
    /// Settings.
    pub fn config(&self) -> &AiConfig {
        &self.config
    }
    pub(crate) fn programs_iter(&self) -> impl Iterator<Item = (&String, &Arc<Program>)> {
        self.programs.iter()
    }
}

/// Inspector view of one brain entity.
pub fn view(world: &World, entity: Entity) -> Option<BrainView> {
    let ai = world.get_resource::<AiWorld>()?;
    let state = world.get::<BrainState>(entity)?;
    let program = ai.programs.get(&state.program)?;
    Some(BrainView::new(
        program,
        state,
        world.get::<Perception>(entity),
        ai.config.fixed_dt,
    ))
}

/// Installs resources and systems on a fixed schedule around `NavigationTick`.
/// Call `ara_nav::register_sim` on the same schedule too.
pub fn register_sim(world: &mut World, schedule: &mut Schedule, config: AiConfig) {
    world.insert_resource(AiWorld::new(config));
    world.insert_resource(PerceptionConfig {
        fixed_dt: config.fixed_dt,
    });
    world.init_resource::<NoiseEvents>();
    schedule.add_systems(
        (sync, perceive, think, bridge)
            .chain()
            .before(NavigationTick),
    );
    schedule.add_systems(
        (locomotion, crate::why::report)
            .chain()
            .after(NavigationTick),
    );
}

/// Copies navigation positions into perception poses; facing follows the look
/// intent, else the movement direction.
pub fn sync(world: &mut World) {
    let mut q = world.query::<(
        &mut PerceptionPose,
        &NavPosition,
        &NavAgentState,
        Option<&LookIntent>,
    )>();
    for (mut pose, position, state, look) in q.iter_mut(world) {
        pose.position = position.0;
        let dir = match look.and_then(|l| l.target) {
            Some(t) => [t[0] - position.0[0], 0.0, t[2] - position.0[2]],
            None => [state.velocity[0], 0.0, state.velocity[2]],
        };
        if dir[0] * dir[0] + dir[2] * dir[2] > 1e-4 {
            pose.forward = dir;
        }
    }
}

fn with_sight<R>(world: &mut World, f: impl FnOnce(&mut World, &dyn LineOfSight) -> R) -> R {
    let Some(ai) = world.remove_resource::<AiWorld>() else {
        return f(world, &OpenSpace);
    };
    let nav_los = match ai.sight {
        Sight::Navmesh(profile) => world
            .get_resource::<NavWorld>()
            .and_then(|n| n.mesh(profile).cloned())
            .map(NavLineOfSight::new),
        _ => None,
    };
    let result = match (&ai.sight, &nav_los) {
        (_, Some(los)) => f(world, los),
        (Sight::Custom(los), None) => f(world, los.as_ref()),
        _ => f(world, &OpenSpace),
    };
    world.insert_resource(ai);
    result
}

/// Runs perception with the AI sight policy.
pub fn perceive(world: &mut World) {
    with_sight(world, |world, los| ara_perception::sense(world, los));
}

type Row<'w> = (
    Mut<'w, BrainState>,
    &'w PerceptionPose,
    Option<&'w Perception>,
    Option<&'w NavAgentState>,
    Option<&'w NavAgent>,
    Option<&'w Brain>,
    Mut<'w, MoveIntent>,
    Mut<'w, LookIntent>,
    Mut<'w, ActionIntent>,
);

/// Advances every enabled brain in parallel; trace events are merged by brain ID.
pub fn think(world: &mut World) {
    let Some(mut ai) = world.remove_resource::<AiWorld>() else {
        return;
    };
    let nav = world.remove_resource::<NavWorld>();
    ai.trace.clear();
    ai.errors.clear();
    ai.poses.clear();
    let poses: Vec<(u64, [f32; 3])> = world
        .query::<&PerceptionPose>()
        .iter(world)
        .map(|p| (p.id, p.position))
        .collect();
    ai.poses = poses;
    ai.poses.sort_by_key(|p| p.0);
    let nav_los = match ai.sight {
        Sight::Navmesh(profile) => nav
            .as_ref()
            .and_then(|n| n.mesh(profile).cloned())
            .map(NavLineOfSight::new),
        _ => None,
    };
    let los: &dyn LineOfSight = match (&ai.sight, &nav_los) {
        (_, Some(los)) => los,
        (Sight::Custom(los), None) => los.as_ref(),
        _ => &OpenSpace,
    };
    let mut query = world.query::<(
        &mut BrainState,
        &PerceptionPose,
        Option<&Perception>,
        Option<&NavAgentState>,
        Option<&NavAgent>,
        Option<&Brain>,
        &mut MoveIntent,
        &mut LookIntent,
        &mut ActionIntent,
    )>();
    let mut rows: Vec<Row<'_>> = query.iter_mut(world).collect();
    let dt = ai.config.fixed_dt;
    let programs = &ai.programs;
    let poses = &ai.poses;
    let nav_ref = nav.as_ref();
    let results: Vec<(Vec<TraceEvent>, Vec<(u64, Diagnostic)>)> = rows
        .par_chunks_mut(32)
        .map(|chunk| {
            let mut trace = Vec::new();
            let mut errors = Vec::new();
            let mut queries: Vec<(u32, Option<ara_nav::NavQuery<'_>>)> = Vec::new();
            for (state, pose, perception, nav_state, agent, brain, moves, look, action) in
                chunk.iter_mut()
            {
                if brain.is_some_and(|b| !b.enabled) {
                    continue;
                }
                let Some(program) = programs.get(&state.program) else {
                    errors.push((
                        state.id,
                        Diagnostic::error(
                            codes::NO_BEHAVIOR,
                            format!("brain runs unknown behaviour `{}`", state.program),
                        ),
                    ));
                    continue;
                };
                let profile = agent.map_or(0, |a| a.profile);
                let mesh = nav_ref.and_then(|n| n.mesh(profile));
                let slot = match queries.iter().position(|(p, _)| *p == profile) {
                    Some(i) => i,
                    None => {
                        queries.push((profile, None));
                        queries.len() - 1
                    }
                };
                let state: &mut BrainState = state;
                let overrides = if state.hash != program.hash || state.active.is_empty() {
                    let (values, problems) =
                        overrides(program, brain.map_or(&[], |b| &b.overrides));
                    errors.extend(problems.into_iter().map(|d| (state.id, d)));
                    values
                } else {
                    Vec::new()
                };
                let mut rng = state.rng;
                let mut env = Env {
                    program,
                    dt,
                    me: state.id,
                    position: pose.position,
                    perception: *perception,
                    nav: *nav_state,
                    poses,
                    los,
                    mesh,
                    query: &mut queries[slot].1,
                    rng: &mut rng,
                    time: 0.0,
                    item: None,
                };
                let mut out = Out {
                    moves,
                    look,
                    action,
                    trace: &mut trace,
                };
                think_brain(&mut env, state, &overrides, &mut out);
                state.rng = rng;
            }
            (trace, errors)
        })
        .collect();
    drop(rows);
    for (trace, errors) in results {
        ai.trace.extend(trace);
        ai.errors.extend(errors);
    }
    ai.trace.sort_by_key(|e| match e {
        TraceEvent::Transition { brain, .. }
        | TraceEvent::Scores { brain, .. }
        | TraceEvent::Eqs { brain, .. }
        | TraceEvent::Task { brain, .. }
        | TraceEvent::Act { brain, .. }
        | TraceEvent::Emit { brain, .. } => *brain,
    });
    ai.errors.sort_by_key(|e| e.0);
    if let Some(nav) = nav {
        world.insert_resource(nav);
    }
    world.insert_resource(ai);
}

/// Parses `name = expression` overrides into blackboard values (constants only).
pub(crate) fn overrides(
    program: &Program,
    lines: &[String],
) -> (Vec<(u16, Value)>, Vec<Diagnostic>) {
    let mut values = Vec::new();
    let mut problems = Vec::new();
    for line in lines {
        let Some((name, text)) = line.split_once('=') else {
            problems.push(Diagnostic::error(
                codes::SYNTAX,
                format!("override `{line}` needs `name = value`"),
            ));
            continue;
        };
        let name = name.trim();
        let Some(slot) = program.slot(name) else {
            problems.push(Diagnostic::error(
                codes::NAME,
                format!("override names unknown variable `{name}`"),
            ));
            continue;
        };
        match dsl::parse_expr(text.trim()).map(|e| constant(&e)) {
            Ok(Some(v)) if fits(&v, program.slots[slot as usize].ty) => values.push((slot, v)),
            Ok(_) => problems.push(Diagnostic::error(
                codes::TYPE,
                format!(
                    "override `{name}` must be a constant {}",
                    program.slots[slot as usize].ty.name()
                ),
            )),
            Err(d) => problems.push(d),
        }
    }
    (values, problems)
}

fn constant(e: &Expr) -> Option<Value> {
    Some(match e {
        Expr::Num(n) => Value::Float(*n as f32),
        Expr::Bool(b) => Value::Bool(*b),
        Expr::None => Value::None,
        Expr::Vec3(parts) => {
            let mut out = [0.0f32; 3];
            for (o, p) in out.iter_mut().zip(parts.iter()) {
                let Value::Float(f) = constant(p)? else {
                    return None;
                };
                *o = f;
            }
            Value::Vec3(out)
        }
        Expr::List(items) => Value::Points(
            items
                .iter()
                .map(|i| match constant(i)? {
                    Value::Vec3(p) => Some(p),
                    _ => None,
                })
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    })
}

fn fits(v: &Value, ty: crate::ast::Type) -> bool {
    use crate::ast::Type;
    matches!(
        (v, ty),
        (Value::Float(_), Type::Float)
            | (Value::Bool(_), Type::Bool)
            | (Value::Vec3(_) | Value::None, Type::Vec3)
            | (Value::None, Type::Entity)
            | (Value::Points(_), Type::Points)
    )
}

/// Sends move intents to navigation: new targets replan, speeds cap the agent.
pub fn bridge(world: &mut World) {
    let mut q = world.query::<(&MoveIntent, &mut NavAgentState)>();
    for (intent, mut state) in q.iter_mut(world) {
        if state.speed_limit != intent.speed {
            state.speed_limit = intent.speed;
        }
        let changed = match (intent.target, state.target) {
            (Some(a), Some(b)) => {
                (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2) > 0.25 * 0.25
            }
            (None, None) => false,
            _ => true,
        };
        if changed {
            state.set_target(intent.target);
        }
    }
}

/// Publishes locomotion intents for animation after navigation moved agents.
pub fn locomotion(world: &mut World) {
    let mut q = world.query::<(
        &NavAgentState,
        Option<&LookIntent>,
        &NavPosition,
        &mut LocomotionIntent,
    )>();
    for (state, look, position, mut intent) in q.iter_mut(world) {
        let facing = match look.and_then(|l| l.target) {
            Some(t) => [t[0] - position.0[0], 0.0, t[2] - position.0[2]],
            None => [state.velocity[0], 0.0, state.velocity[2]],
        };
        let len = (facing[0] * facing[0] + facing[2] * facing[2]).sqrt();
        let next = LocomotionIntent {
            desired: state.desired,
            velocity: state.velocity,
            facing: if len > 1e-3 {
                [facing[0] / len, 0.0, facing[2] / len]
            } else {
                intent.facing
            },
            traversal: state.traversal.map(|t| (t.kind, t.progress())),
        };
        if *intent != next {
            *intent = next;
        }
    }
}

/// Everything an AI agent needs besides its `NavAgent`/`NavPosition`.
pub fn bundle(id: u64, program: &str, position: [f32; 3]) -> impl bevy_ecs::bundle::Bundle {
    (
        BrainState::new(id, program),
        NavAgentState::new(id),
        PerceptionPose {
            id,
            position,
            forward: [0.0, 0.0, 1.0],
        },
        Perception::default(),
        MoveIntent::default(),
        LookIntent::default(),
        ActionIntent::default(),
        LocomotionIntent::default(),
    )
}
