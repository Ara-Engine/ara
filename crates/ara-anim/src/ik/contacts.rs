use super::{Chain, ChainData, ChainSolver, SolverSettings, apply_model_rotation, look_at};
use crate::{Error, Pose, RootMotion, Skeleton, ensure, pose::model_matrices};
use ara_schema::Schema;
use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Deterministically selected closest supporting surface.
#[derive(Debug, Clone, Copy)]
pub struct GroundHit {
    /// World-space contact position.
    pub point: Vec3,
    /// Unit outward world-space normal.
    pub normal: Vec3,
}
/// Narrow scene-query seam. Implementations must use stable hit ordering and fixed scratch.
pub trait GroundQuery {
    /// Casts a ray with a full displacement, excluding the actor in the adapter.
    fn probe(&mut self, origin: Vec3, displacement: Vec3) -> Result<Option<GroundHit>, Error>;
}
/// One model-space full-body contact. Larger priorities are solved last.
#[derive(Debug, Clone, Copy)]
pub struct BodyGoal {
    /// Index into the full-body solver's cooked chains.
    pub chain: usize,
    /// Desired model-space end-effector position.
    pub position: Vec3,
    /// Desired model-space end-effector orientation, when constrained.
    pub rotation: Option<Quat>,
    /// Larger values take precedence; equal values retain caller order.
    pub priority: u8,
    /// Contribution to shared pelvis translation, in [0,1].
    pub pelvis_weight: f32,
}
/// Coupled multi-effector solver with a shared pelvis and ordered contact passes.
/// Runtime goals remain caller-owned ECS data; this object holds only derived scratch.
#[derive(Debug)]
pub struct FullBody {
    skeleton: u64,
    chains: Vec<Chain>,
    pelvis: usize,
    model: Vec<Mat4>,
    order: Vec<usize>,
}
impl FullBody {
    /// Cooks chains and reserves an explicit maximum number of contacts.
    pub fn new(
        skeleton: &Skeleton,
        pelvis: usize,
        chains: &[ChainData],
        max_goals: usize,
    ) -> Result<Self, Error> {
        ensure(
            pelvis < skeleton.len() && max_goals > 0,
            "invalid full-body pelvis or contact capacity",
        )?;
        Ok(Self {
            skeleton: skeleton.signature(),
            chains: chains
                .iter()
                .map(|c| Chain::cook(skeleton, c))
                .collect::<Result<_, _>>()?,
            pelvis,
            model: vec![Mat4::IDENTITY; skeleton.len()],
            order: Vec::with_capacity(max_goals),
        })
    }
    /// Solves hips, hands and feet in bounded deterministic priority order.
    /// Call look_at for the head after contacts; no hidden world or clock is consulted.
    pub fn solve(
        &mut self,
        skeleton: &Skeleton,
        pose: &mut Pose,
        goals: &[BodyGoal],
        max_pelvis_shift: f32,
        settings: SolverSettings,
    ) -> Result<(), Error> {
        ensure(
            skeleton.signature() == self.skeleton
                && goals.len() <= self.order.capacity()
                && max_pelvis_shift.is_finite()
                && max_pelvis_shift >= 0.0
                && pose.len() == skeleton.len()
                && settings.iterations > 0
                && settings.iterations <= 1024
                && settings.tolerance.is_finite()
                && settings.tolerance >= 0.0
                && goals.iter().all(|g| {
                    g.chain < self.chains.len()
                        && g.position.is_finite()
                        && g.rotation
                            .is_none_or(|q| q.is_finite() && q.is_normalized())
                        && g.pelvis_weight.is_finite()
                        && (0.0..=1.0).contains(&g.pelvis_weight)
                }),
            "invalid full-body goals",
        )?;
        model_matrices(skeleton, pose, &mut self.model)?;
        let mut shift = Vec3::ZERO;
        let mut total = 0.0;
        self.order.clear();
        for (i, goal) in goals.iter().enumerate() {
            let tip = self.chains[goal.chain].endpoints().1;
            shift += (goal.position - self.model[tip].w_axis.truncate()) * goal.pelvis_weight;
            total += goal.pelvis_weight;
            // Insertion ordering avoids stable-sort scratch allocation.
            let at = self
                .order
                .partition_point(|j| goals[*j].priority <= goal.priority);
            self.order.insert(at, i);
        }
        if total > 0.0 {
            shift = (shift / total).clamp_length_max(max_pelvis_shift);
            let local = skeleton.parents[self.pelvis].map_or(shift, |p| {
                self.model[p as usize].inverse().transform_vector3(shift)
            });
            pose.translations[self.pelvis] += glam::Vec3A::from(local);
        }
        // Repeated contact sweeps resolve overlapping torso/limb chains. Highest
        // priority contacts are re-established last on every sweep.
        for _ in 0..4 {
            for &i in &self.order {
                let goal = goals[i];
                let chain = &mut self.chains[goal.chain];
                chain.solve(skeleton, pose, goal.position, ChainSolver::Ccd, settings)?;
                if let Some(q) = goal.rotation {
                    model_matrices(skeleton, pose, &mut self.model)?;
                    apply_model_rotation(skeleton, pose, &self.model, chain.endpoints().1, q)?;
                }
            }
        }
        Ok(())
    }
    /// Applies a bounded head/look goal after the body's position constraints.
    pub fn look(
        &mut self,
        skeleton: &Skeleton,
        pose: &mut Pose,
        head: usize,
        target: Vec3,
        local_forward: Vec3,
        max_angle: f32,
    ) -> Result<(), Error> {
        ensure(head < skeleton.len(), "invalid look joint")?;
        model_matrices(skeleton, pose, &mut self.model)?;
        let head_model = self.model[head];
        let q = look_at(
            head_model.to_scale_rotation_translation().1,
            local_forward,
            target - head_model.w_axis.truncate(),
            max_angle,
            1.0,
        )?;
        apply_model_rotation(skeleton, pose, &self.model, head, q)
    }
}
/// Ground-contact setup, serialized in the same animation asset as its chains.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FootSettings {
    /// Probe start above the animated sole in meters.
    #[ara(default = 0.3)]
    pub probe_up: f32,
    /// Probe reach below the animated sole in meters.
    #[ara(default = 0.6)]
    pub probe_down: f32,
    /// Foot-joint height above the ground in meters.
    pub sole_height: f32,
    /// Maximum pelvis displacement in either vertical direction.
    #[ara(default = 0.3)]
    pub pelvis_limit: f32,
    /// Minimum world-up dot ground normal accepted as walkable.
    #[ara(default = 0.5)]
    pub min_normal_y: f32,
}
impl Default for FootSettings {
    fn default() -> Self {
        Self {
            probe_up: 0.3,
            probe_down: 0.6,
            sole_height: 0.0,
            pelvis_limit: 0.3,
            min_normal_y: 0.5,
        }
    }
}
/// A foot chain and its anatomical sole-up direction.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Foot {
    /// Hip-to-ankle chain.
    pub chain: ChainData,
    /// Unit foot-local sole normal.
    #[ara(default = [0, 1, 0])]
    pub up: [f32; 3],
}
impl Default for Foot {
    fn default() -> Self {
        Self {
            chain: ChainData::default(),
            up: [0.0, 1.0, 0.0],
        }
    }
}
/// Actor-local scratch for slope-aware feet and shared pelvis adjustment.
#[derive(Debug)]
pub struct FootPlacement {
    chains: Vec<Chain>,
    up: Vec<Vec3>,
    hits: Vec<Option<GroundHit>>,
    model: Vec<Mat4>,
    pelvis: usize,
    skeleton: u64,
    settings: FootSettings,
}
impl FootPlacement {
    /// Validates configuration and allocates all probe/solver scratch.
    pub fn new(
        skeleton: &Skeleton,
        pelvis: usize,
        feet: &[Foot],
        settings: FootSettings,
    ) -> Result<Self, Error> {
        ensure(
            pelvis < skeleton.len()
                && !feet.is_empty()
                && [
                    settings.probe_up,
                    settings.probe_down,
                    settings.sole_height,
                    settings.pelvis_limit,
                ]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
                && settings.probe_up + settings.probe_down > 0.0
                && settings.min_normal_y.is_finite()
                && (0.0..=1.0).contains(&settings.min_normal_y)
                && feet.iter().all(|f| {
                    Vec3::from(f.up).is_finite()
                        && (Vec3::from(f.up).length_squared() - 1.0).abs() < 1e-4
                }),
            "invalid foot placement setup",
        )?;
        Ok(Self {
            chains: feet
                .iter()
                .map(|f| Chain::cook(skeleton, &f.chain))
                .collect::<Result<_, _>>()?,
            up: feet.iter().map(|f| f.up.into()).collect(),
            hits: vec![None; feet.len()],
            model: vec![Mat4::IDENTITY; skeleton.len()],
            pelvis,
            skeleton: skeleton.signature(),
            settings,
        })
    }
    /// Probes all feet before modifying the pose, then adjusts pelvis and aligns soles.
    /// `world` is rigid actor-to-world; ground normals are transformed back to model space.
    pub fn solve(
        &mut self,
        skeleton: &Skeleton,
        pose: &mut Pose,
        world: RootMotion,
        query: &mut impl GroundQuery,
    ) -> Result<f32, Error> {
        ensure(
            self.skeleton == skeleton.signature()
                && world.translation.is_finite()
                && world.rotation.is_finite()
                && world.rotation.is_normalized(),
            "invalid foot placement rig or world transform",
        )?;
        model_matrices(skeleton, pose, &mut self.model)?;
        let mut pelvis = f32::INFINITY;
        for (i, chain) in self.chains.iter().enumerate() {
            let tip = self.model[chain.endpoints().1].w_axis.truncate();
            let position = world.translation + world.rotation * tip;
            let hit = query.probe(
                position + Vec3::Y * self.settings.probe_up,
                -Vec3::Y * (self.settings.probe_up + self.settings.probe_down),
            )?;
            ensure(
                hit.is_none_or(|h| {
                    h.point.is_finite()
                        && h.normal.is_finite()
                        && (h.normal.length_squared() - 1.0).abs() < 1e-4
                }),
                "ground query returned an invalid hit",
            )?;
            self.hits[i] = hit.filter(|h| h.normal.y >= self.settings.min_normal_y);
            if let Some(h) = self.hits[i] {
                pelvis = pelvis.min(h.point.y + self.settings.sole_height - position.y);
            }
        }
        if !pelvis.is_finite() {
            return Ok(0.0);
        }
        pelvis = pelvis.clamp(-self.settings.pelvis_limit, self.settings.pelvis_limit);
        let shift = world.rotation.conjugate() * (Vec3::Y * pelvis);
        let local = skeleton.parents[self.pelvis].map_or(shift, |p| {
            self.model[p as usize].inverse().transform_vector3(shift)
        });
        pose.translations[self.pelvis] += glam::Vec3A::from(local);
        for (i, chain) in self.chains.iter_mut().enumerate() {
            if let Some(hit) = self.hits[i] {
                let target = world.rotation.conjugate()
                    * (hit.point + Vec3::Y * self.settings.sole_height - world.translation);
                let tip = chain.endpoints().1;
                let original_rotation = self.model[tip].to_scale_rotation_translation().1;
                let normal = world.rotation.conjugate() * hit.normal;
                let aligned = Quat::from_rotation_arc(original_rotation * self.up[i], normal)
                    * original_rotation;
                chain.solve(
                    skeleton,
                    pose,
                    target,
                    ChainSolver::Fabrik,
                    SolverSettings::default(),
                )?;
                // A straight limb can be singular for FABRIK; CCD adds a second deterministic projection.
                chain.solve(
                    skeleton,
                    pose,
                    target,
                    ChainSolver::Ccd,
                    SolverSettings::default(),
                )?;
                model_matrices(skeleton, pose, &mut self.model)?;
                apply_model_rotation(skeleton, pose, &self.model, tip, aligned.normalize())?;
            }
        }
        Ok(pelvis)
    }
}
/// Prop-relative rigid grip. Both transforms are explicit fixed-tick input.
#[derive(Debug, Clone, Copy)]
pub struct HandGrip {
    /// World-space prop transform.
    pub prop: RootMotion,
    /// Desired hand transform in prop space.
    pub local: RootMotion,
}
impl HandGrip {
    /// Converts the grip to an actor-model-space hand target.
    pub fn target(self, actor: RootMotion) -> RootMotion {
        actor.inverse().compose(self.prop).compose(self.local)
    }
    /// Solves hand position and orientation using caller-owned chain/model scratch.
    pub fn solve(
        self,
        actor: RootMotion,
        chain: &mut Chain,
        skeleton: &Skeleton,
        pose: &mut Pose,
        model: &mut [Mat4],
    ) -> Result<(), Error> {
        ensure(
            model.len() == skeleton.len(),
            "hand IK model buffer mismatch",
        )?;
        let target = self.target(actor);
        chain.solve(
            skeleton,
            pose,
            target.translation,
            ChainSolver::Ccd,
            SolverSettings::default(),
        )?;
        model_matrices(skeleton, pose, model)?;
        apply_model_rotation(skeleton, pose, model, chain.endpoints().1, target.rotation)
    }
}
