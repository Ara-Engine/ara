use crate::{Error, Pose, Transform, data::SkeletonData, ensure, pose::model_matrices, signature};
use glam::Mat4;
use std::collections::BTreeSet;

/// Validated immutable skeleton, including inverse bind matrices and retarget data.
#[derive(Debug)]
pub struct Skeleton {
    pub(crate) data: SkeletonData,
    pub(crate) parents: Vec<Option<u32>>,
    pub(crate) bind: Pose,
    pub(crate) inverse_bind: Vec<Mat4>,
    pub(crate) signature: u64,
}
impl Skeleton {
    /// Checks topology, identities and finite transforms, and builds bind matrices.
    pub fn cook(data: SkeletonData) -> Result<Self, Error> {
        ensure(
            !data.joints.is_empty() && (data.motion_root as usize) < data.joints.len(),
            "invalid skeleton size or motion root",
        )?;
        ensure(
            data.joints[data.motion_root as usize].parent.is_none(),
            "motion root must be a hierarchy root",
        )?;
        ensure(
            data.inverse_bind.is_empty() || data.inverse_bind.len() == data.joints.len(),
            "inverse bind matrix count must match the skeleton",
        )?;
        let mut ids = BTreeSet::new();
        let mut bind = Pose::new(data.joints.len());
        let mut parents = Vec::with_capacity(data.joints.len());
        for (i, joint) in data.joints.iter().enumerate() {
            ensure(
                !joint.id.is_empty() && ids.insert(&joint.id),
                "joint IDs must be nonempty and unique",
            )?;
            ensure(
                joint.parent.is_none_or(|p| (p as usize) < i),
                "skeleton parents must precede their children",
            )?;
            let transform = Transform::from(joint.bind);
            ensure(
                transform.valid()
                    && Transform::from(joint.retarget.reference).valid()
                    && joint.retarget.length.is_finite()
                    && joint.retarget.length >= 0.0,
                "invalid bind or retarget transform",
            )?;
            bind.set(i, transform);
            parents.push(joint.parent);
        }
        let signature = signature(&postcard::to_allocvec(&data).map_err(|e| Error(e.to_string()))?);
        let mut result = Self {
            inverse_bind: vec![Mat4::IDENTITY; bind.len()],
            data,
            parents,
            bind,
            signature,
        };
        let mut model = vec![Mat4::IDENTITY; result.len()];
        model_matrices(&result, &result.bind, &mut model)?;
        for (i, (out, bind)) in result.inverse_bind.iter_mut().zip(model).enumerate() {
            ensure(
                bind.is_finite()
                    && bind.determinant().is_finite()
                    && bind.determinant().abs() > 1e-12,
                "singular skeleton bind matrix",
            )?;
            *out = result.data.inverse_bind.get(i).map_or_else(
                || bind.inverse(),
                |m| Mat4::from_cols_array_2d(&[m.x, m.y, m.z, m.w]),
            );
            ensure(
                out.is_finite() && out.determinant().is_finite() && out.determinant().abs() > 1e-12,
                "invalid inverse bind matrix",
            )?;
        }
        Ok(result)
    }
    /// Number of joints in the cooked skeleton.
    pub fn len(&self) -> usize {
        self.parents.len()
    }
    /// Always false for a successfully cooked skeleton.
    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }
    /// Immutable source metadata, including stable IDs and retarget reference axes.
    pub fn data(&self) -> &SkeletonData {
        &self.data
    }
    /// Local bind pose, also the default additive reference.
    pub fn bind_pose(&self) -> &Pose {
        &self.bind
    }
    /// Exact content signature used to reject incompatible clips and snapshots.
    pub fn signature(&self) -> u64 {
        self.signature
    }
}
