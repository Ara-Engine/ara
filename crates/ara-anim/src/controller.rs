pub use crate::compile::Controller;
use crate::{
    Error, Pose, RootMotion, Sampler, Skeleton, ensure,
    graph::{Comparison, ParameterId, ParameterValue},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Lane {
    pub state: usize,
    pub time: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Fade {
    pub target: Lane,
    pub elapsed: u32,
    pub duration: u32,
    pub phase_remainder: u32,
}

/// Complete authoritative animation state, independent of sampling caches.
/// Serialize this alongside the entity's simulation components for rollback.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub(crate) program: u64,
    pub(crate) lane: Lane,
    pub(crate) fade: Option<Fade>,
    pub(crate) parameters: Vec<ParameterValue>,
    pub(crate) tick: u64,
}
impl Clone for Snapshot {
    fn clone(&self) -> Self {
        Self {
            program: self.program,
            lane: self.lane,
            fade: self.fade,
            parameters: self.parameters.clone(),
            tick: self.tick,
        }
    }
    fn clone_from(&mut self, source: &Self) {
        self.program = source.program;
        self.lane = source.lane;
        self.fade = source.fade;
        self.parameters.clone_from(&source.parameters);
        self.tick = source.tick;
    }
}

/// An event occurrence. Identity is (simulation tick, lane, clip, notify ID, cycle).
#[derive(Debug, Clone, Copy)]
pub struct Event<'a> {
    /// Simulation tick at which this occurrence was evaluated.
    pub tick: u64,
    /// Source (0) or target (1) lane during a cross-fade.
    pub lane: u8,
    /// Cooked clip binding index.
    pub clip: usize,
    /// Stable source notify and its gameplay payload.
    pub notify: &'a crate::data::Notify,
    /// Unwrapped loop index, for repeated occurrences.
    pub cycle: u64,
    /// Graph contribution, multiplied by the cross-fade's end-of-tick weight.
    pub weight: f32,
}

#[derive(Debug)]
pub(crate) struct Scratch {
    pub pose: Pose,
    pub curves: Vec<f32>,
    pub sampled_curves: Vec<f32>,
    pub root: RootMotion,
}

/// Per-entity playback state and preallocated derived buffers. Keep this in ECS.
/// Programs and cooked assets are shared immutable resources, never cloned per tick.
#[derive(Debug)]
pub struct Animator {
    pub(crate) state: Snapshot,
    pub(crate) scratch: Vec<Scratch>,
    pub(crate) samplers: Vec<Option<Sampler>>,
    pub(crate) event_weights: Vec<f32>,
    pub(crate) output: Pose,
    pub(crate) curves: Vec<f32>,
}
impl Animator {
    /// Allocates all actor buffers and initializes typed parameters and bind pose.
    pub fn new(controller: &Controller, skeleton: &Skeleton) -> Result<Self, Error> {
        ensure(
            controller.skeleton == skeleton.signature(),
            "animator skeleton mismatch",
        )?;
        let mut scratch = Vec::with_capacity(controller.nodes.len());
        let mut samplers = Vec::with_capacity(controller.nodes.len());
        for node in &controller.nodes {
            let clip = if node.data.kind == crate::graph::NodeKind::Clip {
                Some(&controller.clips[node.clip])
            } else {
                None
            };
            scratch.push(Scratch {
                pose: Pose::new(skeleton.len()),
                curves: vec![0.0; controller.properties.len()],
                sampled_curves: vec![0.0; clip.map_or(0, |c| c.curve_count())],
                root: RootMotion::default(),
            });
            samplers.push(clip.map(Sampler::new));
        }
        let mut actor = Self {
            state: Snapshot {
                program: controller.signature,
                lane: Lane {
                    state: controller.initial,
                    time: 0,
                },
                fade: None,
                parameters: controller.defaults.clone(),
                tick: 0,
            },
            scratch,
            samplers,
            event_weights: vec![0.0; controller.nodes.len()],
            output: skeleton.bind.clone(),
            curves: vec![0.0; controller.properties.len()],
        };
        actor.render(
            controller,
            skeleton,
            actor.state.lane,
            actor.state.lane,
            None,
            &mut |_| {},
        )?;
        Ok(actor)
    }
    /// Sets a typed finite parameter; mismatched types leave state unchanged.
    pub fn set(&mut self, parameter: ParameterId, value: ParameterValue) -> Result<(), Error> {
        let current = self
            .state
            .parameters
            .get_mut(parameter.0 as usize)
            .ok_or_else(|| Error("parameter index does not exist".into()))?;
        ensure(
            value.valid() && current.kind() == value.kind(),
            "parameter value has the wrong type or is not finite",
        )?;
        *current = value;
        Ok(())
    }
    /// Reads a typed parameter by its resolved index.
    pub fn get(&self, parameter: ParameterId) -> Option<ParameterValue> {
        self.state.parameters.get(parameter.0 as usize).copied()
    }
    /// Borrows the latest local pose, with root motion removed if configured.
    pub fn pose(&self) -> &Pose {
        &self.output
    }
    /// Borrows scalar outputs in Controller::curve_properties order.
    pub fn curves(&self) -> &[f32] {
        &self.curves
    }
    /// Creates rollback storage. Use snapshot_into to reuse it on subsequent ticks.
    pub fn snapshot(&self) -> Snapshot {
        self.state.clone()
    }
    /// Copies authoritative state into preallocated rollback storage.
    pub fn snapshot_into(&self, out: &mut Snapshot) {
        out.clone_from(&self.state);
    }
    /// Restores only a compatible checked snapshot and rebuilds derived outputs.
    /// Does not redeliver events or apply root displacement.
    pub fn restore(
        &mut self,
        controller: &Controller,
        skeleton: &Skeleton,
        snapshot: &Snapshot,
    ) -> Result<(), Error> {
        self.check(controller, skeleton)?;
        let valid_lane = |lane: Lane| {
            controller
                .states
                .get(lane.state)
                .is_some_and(|s| s.looping || lane.time <= u64::from(s.duration))
        };
        ensure(
            snapshot.program == controller.signature
                && snapshot.parameters.len() == controller.defaults.len()
                && snapshot
                    .parameters
                    .iter()
                    .zip(&controller.defaults)
                    .all(|(a, b)| a.valid() && a.kind() == b.kind())
                && valid_lane(snapshot.lane)
                && snapshot.fade.is_none_or(|f| {
                    f.duration > 0
                        && f.elapsed < f.duration
                        && valid_lane(f.target)
                        && if synchronized(controller, snapshot.lane.state, f.target.state) {
                            f.phase_remainder < controller.states[snapshot.lane.state].duration
                        } else {
                            f.phase_remainder == 0
                        }
                }),
            "invalid or incompatible animation snapshot",
        )?;
        self.state.clone_from(snapshot);
        for sampler in self.samplers.iter_mut().flatten() {
            sampler.reset();
        }
        self.render(
            controller,
            skeleton,
            self.state.lane,
            self.state.lane,
            self.state.fade.map(|f| (f.target, f.target)),
            &mut |_| {},
        )?;
        Ok(())
    }
    /// Advances exactly one configured simulation tick. Transition selection occurs
    /// before advancement; active fades finish before another transition may start.
    pub fn tick<'a>(
        &mut self,
        controller: &'a Controller,
        skeleton: &Skeleton,
        mut emit: impl FnMut(Event<'a>),
    ) -> Result<RootMotion, Error> {
        #[cfg(feature = "profiling")]
        let _profile = ara_profile::scope("anim.tick");
        self.check(controller, skeleton)?;
        ensure(
            self.state.tick < u64::MAX
                && self.state.lane.time <= u64::MAX - u64::from(controller.step)
                && self.state.fade.is_none_or(|f| {
                    f.target.time
                        <= u64::MAX
                            - target_increment(
                                controller,
                                self.state.lane.state,
                                f,
                                controller.step,
                            )
                            .0
                }),
            "animation clock overflow",
        )?;
        if self.state.fade.is_none() {
            let state = &controller.states[self.state.lane.state];
            let local = if state.looping {
                self.state.lane.time % u64::from(state.duration)
            } else {
                self.state.lane.time
            };
            if let Some(t) = controller.transitions.iter().find(|t| {
                t.from == self.state.lane.state
                    && t.exit_tick
                        .is_none_or(|exit| self.state.lane.time >= u64::from(exit))
                    && t.conditions
                        .iter()
                        .all(|c| c.matches(&self.state.parameters))
            }) {
                let target = &controller.states[t.to];
                let sync = synchronized(controller, self.state.lane.state, t.to);
                let numerator = if sync {
                    // Keep phase within this cycle; avoid unbounded multiplication.
                    local * u64::from(target.duration)
                } else {
                    0
                };
                let time = numerator / u64::from(state.duration);
                let phase_remainder = (numerator % u64::from(state.duration)) as u32;
                for c in &t.conditions {
                    if c.comparison == Comparison::Trigger {
                        self.state.parameters[c.parameter] = ParameterValue::Trigger(false);
                    }
                }
                let lane = Lane { state: t.to, time };
                if t.duration == 0 {
                    self.state.lane = lane;
                } else {
                    self.state.fade = Some(Fade {
                        target: lane,
                        elapsed: 0,
                        duration: t.duration,
                        phase_remainder,
                    });
                }
            }
        }
        let previous = self.state.lane;
        let previous_target = self.state.fade.map(|f| f.target);
        advance(&mut self.state.lane, controller);
        if let Some(fade) = &mut self.state.fade {
            let delta = (self.state.lane.time - previous.time) as u32;
            let (amount, remainder) =
                target_increment(controller, self.state.lane.state, *fade, delta);
            fade.target.time += amount;
            let target = &controller.states[fade.target.state];
            if !target.looping {
                fade.target.time = fade.target.time.min(u64::from(target.duration));
            }
            fade.phase_remainder = remainder;
            fade.elapsed = fade
                .elapsed
                .saturating_add(controller.step)
                .min(fade.duration);
        }
        self.state.tick += 1;
        let root = self.render(
            controller,
            skeleton,
            previous,
            self.state.lane,
            previous_target.zip(self.state.fade.map(|f| f.target)),
            &mut emit,
        )?;
        if let Some(fade) = self.state.fade.filter(|f| f.elapsed == f.duration) {
            self.state.lane = fade.target;
            self.state.fade = None;
        }
        Ok(root)
    }
    fn check(&self, controller: &Controller, skeleton: &Skeleton) -> Result<(), Error> {
        ensure(
            self.state.program == controller.signature
                && skeleton.signature() == controller.skeleton,
            "animator program or skeleton generation mismatch",
        )
    }
}
fn advance(lane: &mut Lane, controller: &Controller) {
    let state = &controller.states[lane.state];
    lane.time = lane.time.saturating_add(u64::from(controller.step));
    if !state.looping {
        lane.time = lane.time.min(u64::from(state.duration));
    }
}

fn synchronized(controller: &Controller, source: usize, target: usize) -> bool {
    let group = &controller.states[source].sync_group;
    !group.is_empty() && *group == controller.states[target].sync_group
}

fn target_increment(controller: &Controller, source: usize, fade: Fade, delta: u32) -> (u64, u32) {
    if !synchronized(controller, source, fade.target.state) {
        return (u64::from(controller.step), 0);
    }
    let duration = u64::from(controller.states[source].duration);
    let numerator = u64::from(delta) * u64::from(controller.states[fade.target.state].duration)
        + u64::from(fade.phase_remainder);
    (numerator / duration, (numerator % duration) as u32)
}
