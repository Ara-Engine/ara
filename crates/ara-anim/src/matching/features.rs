use super::{MatchConfig, TrajectoryPoint};
use crate::{Error, Pose, Skeleton, ensure, pose::model_matrices};
use glam::{Mat4, Vec3};

/// Preallocated model-space feature extraction shared by cooking and runtime queries.
#[derive(Debug)]
pub struct FeatureWriter {
    skeleton: u64,
    root: usize,
    bones: Vec<usize>,
    horizons: usize,
    model: Vec<Mat4>,
    previous: Vec<Mat4>,
}
impl FeatureWriter {
    /// Resolves stable feature bone IDs and allocates FK scratch outside ticks.
    pub fn new(config: &MatchConfig, skeleton: &Skeleton) -> Result<Self, Error> {
        let bones = config
            .bones
            .iter()
            .map(|id| {
                skeleton
                    .data
                    .joints
                    .iter()
                    .position(|j| &j.id == id)
                    .ok_or_else(|| Error(format!("motion feature joint {id} is missing")))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            skeleton: skeleton.signature(),
            root: skeleton.data.motion_root as usize,
            bones,
            horizons: config.horizons.len(),
            model: vec![Mat4::IDENTITY; skeleton.len()],
            previous: vec![Mat4::IDENTITY; skeleton.len()],
        })
    }
    /// Writes root-relative bone positions/velocities, local root velocity and future trajectory.
    #[allow(clippy::too_many_arguments)]
    pub fn write(
        &mut self,
        skeleton: &Skeleton,
        pose: &Pose,
        previous: &Pose,
        dt: f32,
        local_velocity: Vec3,
        trajectory: &[TrajectoryPoint],
        out: &mut [f32],
    ) -> Result<(), Error> {
        ensure(
            skeleton.signature() == self.skeleton
                && dt.is_finite()
                && dt > 0.0
                && trajectory.len() == self.horizons
                && out.len() == self.bones.len() * 6 + 3 + self.horizons * 6
                && local_velocity.is_finite()
                && trajectory
                    .iter()
                    .all(|t| t.position.is_finite() && t.facing.is_finite()),
            "invalid feature extraction inputs",
        )?;
        model_matrices(skeleton, pose, &mut self.model)?;
        model_matrices(skeleton, previous, &mut self.previous)?;
        let root = self.model[self.root].inverse();
        let previous_root = self.previous[self.root].inverse();
        let mut offset = 0;
        for &bone in &self.bones {
            let position = root.transform_point3(self.model[bone].w_axis.truncate());
            let before = previous_root.transform_point3(self.previous[bone].w_axis.truncate());
            out[offset..offset + 3].copy_from_slice(&position.to_array());
            out[offset + 3..offset + 6].copy_from_slice(&((position - before) / dt).to_array());
            offset += 6;
        }
        out[offset..offset + 3].copy_from_slice(&local_velocity.to_array());
        offset += 3;
        for point in trajectory {
            out[offset..offset + 3].copy_from_slice(&point.position.to_array());
            out[offset + 3..offset + 6].copy_from_slice(&point.facing.to_array());
            offset += 6;
        }
        ensure(
            out.iter().all(|v| v.is_finite()),
            "invalid feature pose or arithmetic overflow",
        )
    }
}
