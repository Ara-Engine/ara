//! SIMD transform streams, blends, rigid root motion and skinning palettes.

use crate::{Error, Skeleton, data::AnimTransform, ensure};
use glam::{Mat4, Quat, Vec3, Vec3A};
use serde::{Deserialize, Serialize};

/// A local transform held in SIMD-friendly aligned types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Parent-relative translation in meters.
    pub translation: Vec3A,
    /// Unit rotation quaternion.
    pub rotation: Quat,
    /// Positive parent-relative scale.
    pub scale: Vec3A,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3A::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3A::ONE,
        }
    }
}
impl From<AnimTransform> for Transform {
    fn from(t: AnimTransform) -> Self {
        Self {
            translation: t.translation.into(),
            rotation: Quat::from_array(t.rotation),
            scale: t.scale.into(),
        }
    }
}
impl Transform {
    /// Converts the transform to a column-major matrix.
    pub fn matrix(self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            self.scale.into(),
            self.rotation,
            self.translation.into(),
        )
    }
    pub(crate) fn valid(self) -> bool {
        self.translation.is_finite()
            && self.scale.is_finite()
            && self.scale.min_element() > 0.0
            && self.rotation.is_finite()
            && (self.rotation.length_squared() - 1.0).abs() < 1e-4
    }
}

/// Separate contiguous translation, rotation and scale streams, allocated at setup.
#[derive(Debug, PartialEq, Serialize)]
pub struct Pose {
    pub(crate) translations: Vec<Vec3A>,
    pub(crate) rotations: Vec<Quat>,
    pub(crate) scales: Vec<Vec3A>,
}
impl<'de> Deserialize<'de> for Pose {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Streams {
            translations: Vec<Vec3A>,
            rotations: Vec<Quat>,
            scales: Vec<Vec3A>,
        }
        let streams = Streams::deserialize(deserializer)?;
        let pose = Self {
            translations: streams.translations,
            rotations: streams.rotations,
            scales: streams.scales,
        };
        if pose.translations.len() != pose.len()
            || pose.scales.len() != pose.len()
            || !(0..pose.len()).all(|j| pose.joint(j).is_some_and(Transform::valid))
        {
            return Err(serde::de::Error::custom(
                "invalid pose streams or transforms",
            ));
        }
        Ok(pose)
    }
}
impl Clone for Pose {
    fn clone(&self) -> Self {
        Self {
            translations: self.translations.clone(),
            rotations: self.rotations.clone(),
            scales: self.scales.clone(),
        }
    }
    fn clone_from(&mut self, source: &Self) {
        self.translations.clone_from(&source.translations);
        self.rotations.clone_from(&source.rotations);
        self.scales.clone_from(&source.scales);
    }
}
impl Pose {
    /// Allocates an identity pose for `joints` joints; call outside the tick loop.
    pub fn new(joints: usize) -> Self {
        Self {
            translations: vec![Vec3A::ZERO; joints],
            rotations: vec![Quat::IDENTITY; joints],
            scales: vec![Vec3A::ONE; joints],
        }
    }
    /// Number of joints.
    pub fn len(&self) -> usize {
        self.rotations.len()
    }
    /// Whether this pose has no joints.
    pub fn is_empty(&self) -> bool {
        self.rotations.is_empty()
    }
    /// Borrows the SIMD translation stream.
    pub fn translations(&self) -> &[Vec3A] {
        &self.translations
    }
    /// Borrows the SIMD quaternion stream.
    pub fn rotations(&self) -> &[Quat] {
        &self.rotations
    }
    /// Borrows the SIMD scale stream.
    pub fn scales(&self) -> &[Vec3A] {
        &self.scales
    }
    /// Reads one joint, or none if the index is out of bounds.
    pub fn joint(&self, index: usize) -> Option<Transform> {
        Some(Transform {
            translation: *self.translations.get(index)?,
            rotation: self.rotations[index],
            scale: self.scales[index],
        })
    }
    /// Sets one finite transform, leaving the pose unchanged on invalid input.
    pub fn set_joint(&mut self, index: usize, t: Transform) -> Result<(), Error> {
        ensure(
            index < self.len() && t.valid(),
            "invalid pose joint or transform",
        )?;
        self.set(index, t);
        Ok(())
    }
    pub(crate) fn set(&mut self, i: usize, t: Transform) {
        self.translations[i] = t.translation;
        self.rotations[i] = t.rotation;
        self.scales[i] = t.scale;
    }
    /// Copies a pose into equally sized storage without allocation.
    pub fn copy_from(&mut self, other: &Self) -> Result<(), Error> {
        ensure(self.len() == other.len(), "pose size mismatch")?;
        self.translations.copy_from_slice(&other.translations);
        self.rotations.copy_from_slice(&other.rotations);
        self.scales.copy_from_slice(&other.scales);
        Ok(())
    }
    /// Blends toward `other` with an optional per-bone [0,1] mask.
    pub fn blend(&mut self, other: &Self, weight: f32, mask: Option<&[f32]>) -> Result<(), Error> {
        self.check_blend(other, weight, mask)?;
        for i in 0..self.len() {
            let w = weight * mask.map_or(1.0, |m| m[i]);
            self.translations[i] = self.translations[i].lerp(other.translations[i], w);
            self.rotations[i] = nlerp(self.rotations[i], other.rotations[i], w);
            self.scales[i] = self.scales[i].lerp(other.scales[i], w);
        }
        Ok(())
    }
    /// Adds a local-space difference from `reference` to `additive`, with a bone mask.
    pub fn add(
        &mut self,
        additive: &Self,
        reference: &Self,
        weight: f32,
        mask: Option<&[f32]>,
    ) -> Result<(), Error> {
        self.check_blend(additive, weight, mask)?;
        ensure(
            reference.len() == self.len(),
            "additive reference size mismatch",
        )?;
        for i in 0..self.len() {
            let w = weight * mask.map_or(1.0, |m| m[i]);
            self.translations[i] += (additive.translations[i] - reference.translations[i]) * w;
            let delta = reference.rotations[i].conjugate() * additive.rotations[i];
            self.rotations[i] = (self.rotations[i] * nlerp(Quat::IDENTITY, delta, w)).normalize();
            let ratio = additive.scales[i] / reference.scales[i];
            self.scales[i] *= Vec3A::ONE.lerp(ratio, w);
        }
        Ok(())
    }
    fn check_blend(&self, other: &Self, w: f32, mask: Option<&[f32]>) -> Result<(), Error> {
        ensure(
            self.len() == other.len() && w.is_finite() && (0.0..=1.0).contains(&w),
            "invalid blend size or weight",
        )?;
        ensure(
            mask.is_none_or(|m| {
                m.len() == self.len() && m.iter().all(|w| w.is_finite() && (0.0..=1.0).contains(w))
            }),
            "invalid bone mask",
        )
    }
}

#[inline]
pub(crate) fn nlerp(a: Quat, b: Quat, weight: f32) -> Quat {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    (a * (1.0 - weight) + b * weight).normalize()
}

/// Converts a local pose to model-space matrices in parent-before-child order.
pub fn model_matrices(skeleton: &Skeleton, pose: &Pose, out: &mut [Mat4]) -> Result<(), Error> {
    ensure(
        pose.len() == skeleton.len() && out.len() == skeleton.len(),
        "model matrix buffer size mismatch",
    )?;
    for i in 0..skeleton.len() {
        let local = Transform {
            translation: pose.translations[i],
            rotation: pose.rotations[i],
            scale: pose.scales[i],
        }
        .matrix();
        out[i] = skeleton.parents[i].map_or(local, |p| out[p as usize] * local);
    }
    Ok(())
}

/// Writes model-space joint matrices multiplied by inverse bind matrices.
/// Renderer applies the skinned mesh's model transform separately.
pub fn skinning_palette(
    skeleton: &Skeleton,
    model: &[Mat4],
    out: &mut [Mat4],
) -> Result<(), Error> {
    ensure(
        model.len() == skeleton.len() && out.len() == skeleton.len(),
        "palette buffer size mismatch",
    )?;
    for (i, matrix) in out.iter_mut().enumerate() {
        *matrix = model[i] * skeleton.inverse_bind[i];
    }
    Ok(())
}

/// An SE(3) displacement in the root's previous local coordinate system.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RootMotion {
    /// Translation in meters, rotated by the preceding root orientation.
    pub translation: Vec3,
    /// Unit rotational displacement.
    pub rotation: Quat,
}
impl Default for RootMotion {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        }
    }
}
impl RootMotion {
    /// Ignores scale and keeps the transform's rigid part.
    pub fn from_transform(t: Transform) -> Self {
        Self {
            translation: t.translation.into(),
            rotation: t.rotation,
        }
    }
    /// Composes ordered rigid transforms: apply `rhs`, then `self`.
    pub fn compose(self, rhs: Self) -> Self {
        Self {
            translation: self.translation + self.rotation * rhs.translation,
            rotation: (self.rotation * rhs.rotation).normalize(),
        }
    }
    /// Inverts the rigid transform.
    pub fn inverse(self) -> Self {
        let rotation = self.rotation.conjugate();
        Self {
            translation: rotation * -self.translation,
            rotation,
        }
    }
    /// Relative displacement between two root transforms.
    pub fn between(previous: Self, current: Self) -> Self {
        previous.inverse().compose(current)
    }
    /// Blends displacements using the same weights as the pose.
    pub fn blend(self, other: Self, weight: f32) -> Self {
        Self {
            translation: self.translation.lerp(other.translation, weight),
            rotation: nlerp(self.rotation, other.rotation, weight),
        }
    }
    pub(crate) fn pow(mut self, mut count: u64) -> Self {
        let mut out = Self::default();
        while count != 0 {
            if count & 1 != 0 {
                out = out.compose(self);
            }
            self = self.compose(self);
            count >>= 1;
        }
        out
    }
}
