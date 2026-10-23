//! Reflected source data. Importers bake cubic channels into these linear keys.

use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// A local affine transform; rotations use XYZW quaternions.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimTransform {
    /// Translation in meters relative to the parent.
    pub translation: [f32; 3],
    /// Unit rotation quaternion in XYZW order.
    #[ara(default = [0, 0, 0, 1])]
    pub rotation: [f32; 4],
    /// Positive scale relative to the parent.
    #[ara(default = [1, 1, 1])]
    pub scale: [f32; 3],
}
impl Default for AnimTransform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

/// Bone identity and reference pose used by a retargeting tool.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Retarget {
    /// Semantic role such as hips, leftFoot or head; empty for unspecified.
    pub role: String,
    /// Reference orientation defining the source bone's anatomical axes.
    pub reference: AnimTransform,
    /// Bone length in meters; zero means unspecified.
    pub length: f32,
}

/// A joint in parent-before-child order, with stable importer identity.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Joint {
    /// Stable joint identifier, preserved when its display name changes.
    pub id: String,
    /// Human-readable joint name.
    pub name: String,
    /// Index of the parent joint; absent for a root.
    pub parent: Option<u32>,
    /// Local bind transform relative to the parent.
    pub bind: AnimTransform,
    /// Retargeting metadata preserved by the cooked skeleton.
    pub retarget: Retarget,
}

/// An explicitly imported inverse-bind matrix, in column-major order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimMatrix {
    /// First matrix column.
    pub x: [f32; 4],
    /// Second matrix column.
    pub y: [f32; 4],
    /// Third matrix column.
    pub z: [f32; 4],
    /// Fourth matrix column, including translation.
    pub w: [f32; 4],
}

/// An imported skeleton, ordered for a linear model-space traversal.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SkeletonData {
    /// Parent-before-child joints; at least one joint is required.
    pub joints: Vec<Joint>,
    /// Root joint whose rigid motion is extracted by the runtime.
    pub motion_root: u32,
    /// Explicit mesh-space inverse binds, in joint order. Empty derives them from
    /// bind transforms; glTF importers preserve the skin accessor here instead.
    pub inverse_bind: Vec<AnimMatrix>,
}

/// Interpolation between consecutive keys.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(rename_all = "camelCase")]
pub enum Interpolation {
    /// Linear vectors and shortest-path normalized linear quaternions.
    #[default]
    Linear,
    /// Hold the preceding key until the next key's exact timestamp.
    Step,
}

/// A vector key at an exact integer clip time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct VectorKey {
    /// Clip time in ticks.
    pub tick: u32,
    /// Translation in meters or dimensionless scale.
    pub value: [f32; 3],
}

/// A unit quaternion key at an exact integer clip time.
#[derive(Debug, Clone, Copy, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RotationKey {
    /// Clip time in ticks.
    pub tick: u32,
    /// Unit quaternion in XYZW order.
    #[ara(default = [0, 0, 0, 1])]
    pub value: [f32; 4],
}
impl Default for RotationKey {
    fn default() -> Self {
        Self {
            tick: 0,
            value: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// Sparse local transform channels for one joint; empty channels use bind pose.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct JointTrack {
    /// Translation channel.
    pub translations: Vec<VectorKey>,
    /// Rotation channel.
    pub rotations: Vec<RotationKey>,
    /// Scale channel.
    pub scales: Vec<VectorKey>,
    /// Translation interpolation, independent of rotation and scale.
    pub translation_interpolation: Interpolation,
    /// Quaternion interpolation, independent of translation and scale.
    pub rotation_interpolation: Interpolation,
    /// Scale interpolation, independent of translation and rotation.
    pub scale_interpolation: Interpolation,
}

/// A scalar key for a property curve.
#[derive(Debug, Clone, Copy, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ScalarKey {
    /// Clip time in ticks.
    pub tick: u32,
    /// Finite property value in the addressed field's units.
    pub value: f32,
}

/// An animation curve addressing the same property path as the inspector.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Curve {
    /// Stable target property path, such as MorphWeights.smile.
    pub property: String,
    /// Strictly increasing keys; at least one is required.
    pub keys: Vec<ScalarKey>,
    /// Scalar interpolation mode.
    pub interpolation: Interpolation,
}

/// A discrete animation notify, identified independently of its display name.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Notify {
    /// Stable notify ID for prediction event deduplication.
    pub id: String,
    /// Event name delivered to gameplay.
    pub name: String,
    /// Clip time in ticks, in the half-open range [0, duration).
    pub tick: u32,
    /// Schema-addressable event payload encoded as a string.
    pub payload: String,
}

/// An editable clip. Cook this once in the asset worker, never on a frame thread.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipData {
    /// Nonzero duration in integer ticks.
    pub duration: u32,
    /// Clip tick frequency. Controllers require all clips to share this frequency.
    #[ara(default = 48000)]
    pub ticks_per_second: u32,
    /// Exactly one sparse track per skeleton joint, in skeleton order.
    pub tracks: Vec<JointTrack>,
    /// Animated scalar properties, independent of joint channels.
    pub curves: Vec<Curve>,
    /// Notifies in increasing time order; equal-time order is preserved.
    pub notifies: Vec<Notify>,
}
impl Default for ClipData {
    fn default() -> Self {
        Self {
            duration: 0,
            ticks_per_second: 48000,
            tracks: Vec::new(),
            curves: Vec::new(),
            notifies: Vec::new(),
        }
    }
}
