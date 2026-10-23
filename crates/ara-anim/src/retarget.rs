//! Humanoid mapping and bind-frame retargeting, with runtime and offline IK correction.
mod mapping;
pub use mapping::{BoneRole, RigBone, RigMap};

use crate::{
    CookedClip, Error, Pose, Sampler, Skeleton,
    data::{ClipData, JointTrack, RotationKey, VectorKey},
    ensure,
    ik::{Chain, ChainData, ChainSolver, SolverSettings},
    pose::model_matrices,
};
use glam::{Mat4, Quat, Vec3};

/// Precomputed correspondence and caller-owned FK/IK scratch for one target actor.
#[derive(Debug)]
pub struct Retargeter {
    source: u64,
    target: u64,
    pairs: Vec<(usize, usize)>,
    source_bind: Vec<Mat4>,
    target_bind: Vec<Mat4>,
    source_model: Vec<Mat4>,
    target_model: Vec<Mat4>,
    effectors: Vec<(usize, Chain)>,
    scale: f32,
}
impl Retargeter {
    /// Resolves bone roles, reference frames and leg-height scale at import/setup time.
    pub fn cook(
        source: &Skeleton,
        source_rig: &RigMap,
        target: &Skeleton,
        target_rig: &RigMap,
    ) -> Result<Self, Error> {
        source_rig.validate(source)?;
        target_rig.validate(target)?;
        let mut pairs = Vec::new();
        for bone in &target_rig.bones {
            if let (Some(s), Some(t)) = (
                source_rig.joint(source, bone.role),
                target_rig.joint(target, bone.role),
            ) {
                pairs.push((s, t));
            }
        }
        ensure(!pairs.is_empty(), "retarget rigs have no shared roles")?;
        pairs.sort_by_key(|(_, t)| *t);
        let mut source_bind = vec![Mat4::IDENTITY; source.len()];
        let mut target_bind = vec![Mat4::IDENTITY; target.len()];
        model_matrices(source, source.bind_pose(), &mut source_bind)?;
        model_matrices(target, target.bind_pose(), &mut target_bind)?;
        let height = |s: &Skeleton, rig: &RigMap, model: &[Mat4]| -> Option<f32> {
            let h = rig.joint(s, BoneRole::Hips)?;
            let f = rig
                .joint(s, BoneRole::LeftFoot)
                .or_else(|| rig.joint(s, BoneRole::RightFoot))?;
            Some((model[h].w_axis.y - model[f].w_axis.y).abs())
        };
        let scale = match (
            height(source, source_rig, &source_bind),
            height(target, target_rig, &target_bind),
        ) {
            (Some(a), Some(b)) if a > 1e-5 && b > 1e-5 => b / a,
            _ => 1.0,
        };
        let mut effectors = Vec::new();
        for (root, tip) in [
            (BoneRole::LeftUpperLeg, BoneRole::LeftFoot),
            (BoneRole::RightUpperLeg, BoneRole::RightFoot),
            (BoneRole::LeftUpperArm, BoneRole::LeftHand),
            (BoneRole::RightUpperArm, BoneRole::RightHand),
        ] {
            if let (Some(s), Some(t), Some(r)) = (
                source_rig.joint(source, tip),
                target_rig.joint(target, tip),
                target_rig.joint(target, root),
            ) {
                let mut path = vec![t];
                let mut cursor = t;
                while cursor != r {
                    cursor = target.parents[cursor]
                        .ok_or_else(|| Error("retarget limb is not connected".into()))?
                        as usize;
                    path.push(cursor);
                }
                path.reverse();
                let data = ChainData {
                    joints: path
                        .iter()
                        .map(|j| target.data.joints[*j].id.clone())
                        .collect(),
                    ..Default::default()
                };
                effectors.push((s, Chain::cook(target, &data)?));
            }
        }
        Ok(Self {
            source: source.signature(),
            target: target.signature(),
            pairs,
            source_bind,
            target_bind,
            source_model: vec![Mat4::IDENTITY; source.len()],
            target_model: vec![Mat4::IDENTITY; target.len()],
            effectors,
            scale,
        })
    }
    /// Height ratio used for root displacement and end-effector motion.
    pub fn root_scale(&self) -> f32 {
        self.scale
    }
    /// Retargets one pose without allocations. Bind frames preserve differing T/A poses.
    /// `correct_effectors` uses scaled bind-relative targets to preserve planted feet.
    pub fn apply(
        &mut self,
        source: &Skeleton,
        source_pose: &Pose,
        target: &Skeleton,
        out: &mut Pose,
        correct_effectors: bool,
    ) -> Result<(), Error> {
        ensure(
            source.signature() == self.source
                && target.signature() == self.target
                && source_pose.len() == source.len()
                && out.len() == target.len(),
            "retarget skeleton or pose mismatch",
        )?;
        model_matrices(source, source_pose, &mut self.source_model)?;
        out.copy_from(target.bind_pose())?;
        // Translate roots/hips only. Local limb translations retain target proportions.
        for &(s, t) in &self.pairs {
            if source.parents[s].is_none()
                || target.parents[t].is_none()
                || target.data.joints[t].retarget.role == "hips"
            {
                out.translations[t] +=
                    (source_pose.translations[s] - source.bind.translations[s]) * self.scale;
            }
        }
        model_matrices(target, out, &mut self.target_model)?;
        for &(s, t) in &self.pairs {
            let source_rest = self.source_bind[s].to_scale_rotation_translation().1;
            let target_rest = self.target_bind[t].to_scale_rotation_translation().1;
            let desired = self.source_model[s].to_scale_rotation_translation().1
                * source_rest.conjugate()
                * target_rest;
            let parent = target.parents[t].map_or(Quat::IDENTITY, |p| {
                self.target_model[p as usize]
                    .to_scale_rotation_translation()
                    .1
            });
            out.rotations[t] = (parent.conjugate() * desired).normalize();
            // Includes unmapped twist/helper bones between mapped roles.
            model_matrices(target, out, &mut self.target_model)?;
        }
        if correct_effectors {
            for (s, chain) in &mut self.effectors {
                let t = chain.endpoints().1;
                let delta = self.source_model[*s].w_axis.truncate()
                    - self.source_bind[*s].w_axis.truncate();
                let goal = self.target_bind[t].w_axis.truncate() + delta * self.scale;
                // Analytic two-bone initialization escapes straight-chain singularities.
                let r = chain.endpoints().0;
                if let Some(m) =
                    target.parents[t].filter(|m| target.parents[*m as usize] == Some(r as u32))
                {
                    model_matrices(target, out, &mut self.target_model)?;
                    let (a, b) = crate::ik::two_bone(
                        self.target_model[r],
                        self.target_model[m as usize],
                        self.target_model[t],
                        goal,
                        Vec3::Z,
                        1.0,
                    )?;
                    crate::ik::apply_model_rotation(target, out, &self.target_model, r, a)?;
                    model_matrices(target, out, &mut self.target_model)?;
                    crate::ik::apply_model_rotation(
                        target,
                        out,
                        &self.target_model,
                        m as usize,
                        b,
                    )?;
                }
                chain.solve(
                    target,
                    out,
                    goal,
                    ChainSolver::Fabrik,
                    SolverSettings::default(),
                )?;
            }
        }
        Ok(())
    }
    /// Bakes a dense corrected source document; pass it through CookedClip::cook on the import worker.
    /// Curves and notifies retain their exact source timebase and identities.
    pub fn bake(
        &mut self,
        source: &Skeleton,
        clip: &ClipData,
        target: &Skeleton,
        step: u32,
    ) -> Result<ClipData, Error> {
        ensure(step > 0, "retarget bake step must be positive")?;
        let (cooked, _) = CookedClip::cook(clip, source, crate::CompressionSettings::default())?;
        let mut sampler = Sampler::new(&cooked);
        let mut input = Pose::new(source.len());
        let mut output = Pose::new(target.len());
        let mut curves = vec![0.0; cooked.curve_count()];
        let mut result = ClipData {
            duration: clip.duration,
            ticks_per_second: clip.ticks_per_second,
            tracks: vec![JointTrack::default(); target.len()],
            curves: clip.curves.clone(),
            notifies: clip.notifies.clone(),
        };
        let mut tick = 0;
        loop {
            sampler.sample(&cooked, source, tick, &mut input, &mut curves)?;
            self.apply(source, &input, target, &mut output, true)?;
            for (j, track) in result.tracks.iter_mut().enumerate() {
                track.translations.push(VectorKey {
                    tick,
                    value: output.translations[j].to_array(),
                });
                track.rotations.push(RotationKey {
                    tick,
                    value: output.rotations[j].to_array(),
                });
                track.scales.push(VectorKey {
                    tick,
                    value: output.scales[j].to_array(),
                });
            }
            if tick == clip.duration {
                break;
            }
            tick = tick.saturating_add(step).min(clip.duration);
        }
        Ok(result)
    }
}
