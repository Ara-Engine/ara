use super::{Inertializer, MotionDatabase};
use crate::{
    CookedClip, Error, Pose, RootMotion, Sampler, Skeleton,
    compile::{Controller, ProgramMatch},
    graph::ParameterValue,
};
use glam::Vec4;
use serde::{Deserialize, Serialize};

/// Complete motion-node playback and inertial history; included in Animator snapshots.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct MotionState {
    pub initialized: bool,
    pub last_tick: u64,
    pub clip: usize,
    pub time: u64,
    pub before: u64,
    pub root: RootMotion,
    pub pose: Pose,
    pub previous: Pose,
    pub inertia: Inertializer,
}
impl Clone for MotionState {
    fn clone(&self) -> Self {
        Self {
            initialized: self.initialized,
            last_tick: self.last_tick,
            clip: self.clip,
            time: self.time,
            before: self.before,
            root: self.root,
            pose: self.pose.clone(),
            previous: self.previous.clone(),
            inertia: self.inertia.clone(),
        }
    }
    fn clone_from(&mut self, other: &Self) {
        self.initialized = other.initialized;
        self.last_tick = other.last_tick;
        self.clip = other.clip;
        self.time = other.time;
        self.before = other.before;
        self.root = other.root;
        self.pose.clone_from(&other.pose);
        self.previous.clone_from(&other.previous);
        self.inertia.clone_from(&other.inertia);
    }
}
#[derive(Debug)]
pub(crate) struct MotionScratch {
    raw: Vec<f32>,
    normalized: Vec<Vec4>,
    samplers: Vec<Sampler>,
    incoming: Pose,
    prior: Pose,
    pub curves: Vec<Vec<f32>>,
}
impl MotionState {
    pub fn new(program: &ProgramMatch, skeleton: &Skeleton) -> Result<Self, Error> {
        Ok(Self {
            initialized: false,
            last_tick: 0,
            clip: 0,
            time: 0,
            before: 0,
            root: RootMotion::default(),
            pose: skeleton.bind_pose().clone(),
            previous: skeleton.bind_pose().clone(),
            inertia: Inertializer::new(skeleton.len(), program.settings.half_life)?,
        })
    }
    pub fn valid(&self, program: &ProgramMatch, skeleton: &Skeleton) -> bool {
        self.clip < program.clips.len()
            && self.before <= self.time
            && self.pose.len() == skeleton.len()
            && self.previous.len() == skeleton.len()
            && self.pose.translations.len() == skeleton.len()
            && self.pose.scales.len() == skeleton.len()
            && self.previous.translations.len() == skeleton.len()
            && self.previous.scales.len() == skeleton.len()
            && self.inertia.valid(skeleton.len())
            && [&self.pose, &self.previous]
                .iter()
                .all(|p| (0..p.len()).all(|j| p.joint(j).is_some_and(|t| t.valid())))
            && self.root.translation.is_finite()
            && self.root.rotation.is_finite()
            && self.root.rotation.is_normalized()
    }
}
impl MotionScratch {
    pub fn new(
        program: &ProgramMatch,
        db: &MotionDatabase,
        clips: &[CookedClip],
        skeleton: &Skeleton,
    ) -> Self {
        Self {
            raw: vec![0.0; db.config().dimensions()],
            normalized: vec![Vec4::ZERO; db.query_groups()],
            samplers: program
                .clips
                .iter()
                .map(|i| Sampler::new(&clips[*i]))
                .collect(),
            incoming: Pose::new(skeleton.len()),
            prior: Pose::new(skeleton.len()),
            curves: program
                .clips
                .iter()
                .map(|i| vec![0.0; clips[*i].curve_count()])
                .collect(),
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        &mut self,
        state: &mut MotionState,
        program: &ProgramMatch,
        db: &MotionDatabase,
        c: &Controller,
        skeleton: &Skeleton,
        parameters: &[ParameterValue],
        tick: u64,
    ) -> Result<(), Error> {
        if state.initialized && state.last_tick == tick {
            let clip = &c.clips[program.clips[state.clip]];
            self.samplers[state.clip].sample(
                clip,
                skeleton,
                (state.time % u64::from(clip.duration)) as u32,
                &mut self.incoming,
                &mut self.curves[state.clip],
            )?;
            return Ok(());
        }
        for (raw, parameter) in self.raw.iter_mut().zip(&program.parameters) {
            *raw = parameters[*parameter].float();
        }
        db.normalize(&self.raw, &mut self.normalized)?;
        let found = db.search(&self.normalized, program.settings.budget as usize)?;
        let selected = db
            .frame(found.row)
            .expect("search always returns a database row");
        let expected = if state.initialized {
            state
                .time
                .checked_add(u64::from(c.step))
                .ok_or_else(|| Error("motion node clock exhausted".into()))?
        } else {
            0
        };
        let clip = &c.clips[program.clips[state.clip]];
        let distance = (expected % u64::from(clip.duration)).abs_diff(u64::from(selected.tick));
        let jumped = !state.initialized
            || selected.clip != state.clip
            || distance > u64::from(program.settings.continuation);
        let time = if jumped {
            u64::from(selected.tick)
        } else {
            expected
        };
        let binding = if jumped { selected.clip } else { state.clip };
        let clip = &c.clips[program.clips[binding]];
        let local = (time % u64::from(clip.duration)) as u32;
        self.samplers[binding].sample(
            clip,
            skeleton,
            local,
            &mut self.incoming,
            &mut self.curves[binding],
        )?;
        let dt = c.step as f32 / clip.frequency as f32;
        if jumped && state.initialized {
            self.samplers[binding].sample(
                clip,
                skeleton,
                local.saturating_sub(c.step),
                &mut self.prior,
                &mut self.curves[binding],
            )?;
            state.inertia.begin(
                &state.previous,
                &state.pose,
                &self.prior,
                &self.incoming,
                dt,
            )?;
        }
        state.previous.copy_from(&state.pose)?;
        state
            .inertia
            .apply(&mut self.incoming, if jumped { 0.0 } else { dt })?;
        state.pose.copy_from(&self.incoming)?;
        state.before = if jumped { time } else { state.time };
        state.time = time;
        state.clip = binding;
        state.root =
            if jumped {
                RootMotion::default()
            } else {
                let (start, cycle) = clip.root_cache;
                let at = |t| {
                    RootMotion::between(
                        start,
                        clip.root_at(skeleton, (t % u64::from(clip.duration)) as f64),
                    )
                };
                at(state.before)
                    .inverse()
                    .compose(cycle.pow(
                        time / u64::from(clip.duration) - state.before / u64::from(clip.duration),
                    ))
                    .compose(at(time))
            };
        // A prior-pose sample during begin must not replace this tick's curve values.
        if jumped && state.initialized {
            self.samplers[binding].sample(
                clip,
                skeleton,
                local,
                &mut self.incoming,
                &mut self.curves[binding],
            )?;
        }
        state.last_tick = tick;
        state.initialized = true;
        Ok(())
    }
}
