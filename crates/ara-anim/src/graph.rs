//! Canonical animation graph model; stable map keys become ara-graph node IDs.

use ara_schema::{AssetRef, Schema};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A schema-registered gameplay parameter type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimParameterKind", module = "animation")]
#[serde(rename_all = "camelCase")]
pub enum ParameterKind {
    /// Finite floating-point value.
    #[default]
    Float,
    /// Signed 32-bit integer.
    Int,
    /// Boolean state.
    Bool,
    /// Latched trigger, consumed only by a selected transition that tests it.
    Trigger,
}

/// A typed parameter declaration, addressed by its stable map key.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimParameter", module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Parameter {
    /// Parameter's storage and comparison type.
    pub kind: ParameterKind,
    /// Initial value for a float parameter.
    pub float_value: f32,
    /// Initial value for an integer parameter.
    pub int_value: i32,
    /// Initial value for a boolean parameter; triggers always start unset.
    pub bool_value: bool,
}

/// Parameter value accepted by generated C# bindings and prediction snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ParameterValue {
    /// Finite floating-point state.
    Float(f32),
    /// Integer state.
    Int(i32),
    /// Boolean state.
    Bool(bool),
    /// Latched transition trigger.
    Trigger(bool),
}
impl ParameterValue {
    pub(crate) fn valid(self) -> bool {
        !matches!(self, Self::Float(v) if !v.is_finite())
    }
    pub(crate) fn kind(self) -> ParameterKind {
        match self {
            Self::Float(_) => ParameterKind::Float,
            Self::Int(_) => ParameterKind::Int,
            Self::Bool(_) => ParameterKind::Bool,
            Self::Trigger(_) => ParameterKind::Trigger,
        }
    }
    pub(crate) fn float(self) -> f32 {
        if let Self::Float(v) = self { v } else { 0.0 }
    }
}
impl Parameter {
    pub(crate) fn value(&self) -> ParameterValue {
        match self.kind {
            ParameterKind::Float => ParameterValue::Float(self.float_value),
            ParameterKind::Int => ParameterValue::Int(self.int_value),
            ParameterKind::Bool => ParameterValue::Bool(self.bool_value),
            ParameterKind::Trigger => ParameterValue::Trigger(false),
        }
    }
}

/// A parameter index resolved once at setup, never by name on the hot path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterId(pub u32);

/// Runtime pose operation represented by an authoring graph node.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimNodeKind", module = "animation")]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    /// A referenced cooked clip.
    #[default]
    Clip,
    /// Interpolate adjacent samples on an ordered scalar axis.
    Blend1d,
    /// Interpolate a center and an angular ring with radial barycentric weights.
    Directional2d,
    /// Add the second input's difference from the skeleton bind pose to the first.
    Additive,
    /// Blend the second input onto the first through a per-joint mask.
    Layer,
    /// Bounded search in a normalized pose/trajectory feature database.
    MotionMatching,
}

/// A blend sample with a stable child-node identity.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimBlendSample", module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BlendSample {
    /// Stable child-node ID in the same controller.
    pub node: String,
    /// Scalar coordinate, or horizontal directional coordinate.
    pub x: f32,
    /// Vertical directional coordinate; zero for a scalar blend.
    pub y: f32,
}

/// A typed animation graph node. The compiler validates fields for each kind.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimNode", module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    /// Pose operation.
    pub kind: NodeKind,
    /// Clip reference in canonical @path#id form, resolved by ara-asset.
    #[ara(asset_kind = "animation")]
    pub clip: Option<AssetRef>,
    /// Float parameter for a scalar axis, horizontal axis or optional layer weight.
    pub parameter_x: String,
    /// Float parameter for a directional vertical axis.
    pub parameter_y: String,
    /// Samples; additive/layer nodes use exactly two in base, overlay order.
    pub samples: Vec<BlendSample>,
    /// Constant additive/layer weight when parameterX is empty.
    #[ara(default = 1)]
    pub weight: f32,
    /// Per-joint [0,1] weights for layer/additive nodes; empty means every joint.
    pub bone_mask: Vec<f32>,
    /// Search/query/inertialization settings for a motion-matching node.
    pub matching: Option<crate::matching::MatchNode>,
}
impl Default for Node {
    fn default() -> Self {
        Self {
            kind: NodeKind::Clip,
            clip: None,
            parameter_x: String::new(),
            parameter_y: String::new(),
            samples: Vec::new(),
            weight: 1.0,
            bone_mask: Vec::new(),
            matching: None,
        }
    }
}

/// A state selecting a pose graph and its normalized phase clock.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimState", module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct State {
    /// Stable pose graph node ID.
    pub node: String,
    /// Duration in clip ticks; zero infers the longest descendant clip.
    pub duration: u32,
    /// Whether to wrap the phase clock and repeat notifies.
    #[ara(default = true)]
    pub looping: bool,
    /// Nonempty group preserves normalized phase on entry and throughout cross-fades.
    pub sync_group: String,
}
impl Default for State {
    fn default() -> Self {
        Self {
            node: String::new(),
            duration: 0,
            looping: true,
            sync_group: String::new(),
        }
    }
}

/// A typed parameter comparison.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimComparison", module = "animation")]
#[serde(rename_all = "camelCase")]
pub enum Comparison {
    /// Exact equality in the parameter's storage type.
    #[default]
    Equal,
    /// Exact inequality in the parameter's storage type.
    NotEqual,
    /// Strict numeric greater-than.
    Greater,
    /// Strict numeric less-than.
    Less,
    /// A latched trigger is set.
    Trigger,
}

/// One transition predicate; all conditions on a transition must match.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimCondition", module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Condition {
    /// Stable parameter name.
    pub parameter: String,
    /// Comparison valid for that parameter's type.
    pub comparison: Comparison,
    /// Right-hand side for a float comparison.
    pub float_value: f32,
    /// Right-hand side for an integer comparison.
    pub int_value: i32,
    /// Right-hand side for a boolean comparison.
    pub bool_value: bool,
}

/// A state transition. Serialized list order defines deterministic priority.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimTransition", module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Transition {
    /// Stable transition ID for tools and graph mapping.
    pub id: String,
    /// Source state ID.
    pub from: String,
    /// Target state ID.
    pub to: String,
    /// Fixed cross-fade duration; zero switches before advancing this tick.
    pub duration: u32,
    /// Optional earliest elapsed state tick; duration waits for one complete cycle.
    pub exit_tick: Option<u32>,
    /// Typed AND predicates; empty means unconditional.
    pub conditions: Vec<Condition>,
}

/// Canonical controller source, lowered to an immutable program by Controller.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "AnimController", module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerData {
    /// Stable typed parameter declarations.
    pub parameters: BTreeMap<String, Parameter>,
    /// Stable graph nodes; child references must form a DAG.
    pub nodes: BTreeMap<String, Node>,
    /// Stable named states.
    pub states: BTreeMap<String, State>,
    /// Transition list, in descending priority.
    pub transitions: Vec<Transition>,
    /// Initial state ID.
    pub initial: String,
    /// Fixed tick duration in the clips' common timebase.
    #[ara(default = 800)]
    pub fixed_step: u32,
    /// Remove the root's animated rigid transform after extraction.
    #[ara(default = true)]
    pub extract_root_motion: bool,
}
impl Default for ControllerData {
    fn default() -> Self {
        Self {
            parameters: BTreeMap::new(),
            nodes: BTreeMap::new(),
            states: BTreeMap::new(),
            transitions: Vec::new(),
            initial: String::new(),
            fixed_step: 800,
            extract_root_motion: true,
        }
    }
}
