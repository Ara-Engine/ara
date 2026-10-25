use crate::{Module, decode, encode, error};
use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Parameter ownership, separate from the particle state channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Namespace {
    /// Shared by every emitter in the effect.
    System,
    /// Scoped to the named emitter.
    Emitter,
    /// Copied to a particle at initialization, immutable thereafter.
    Particle,
    /// Exposed to gameplay/editor instance overrides.
    User,
}
/// Typed authoring parameter; there is no numeric/boolean coercion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Value {
    /// Scalar shader float.
    Float(f32),
    /// Direction, position or vector, in metres.
    Vec3([f32; 3]),
    /// Scene-linear straight RGBA.
    Color([f32; 4]),
    /// Unsigned counter or seed.
    Uint(u32),
    /// Boolean switch.
    Bool(bool),
}
impl Value {
    pub(crate) fn literal(&self) -> serde_json::Value {
        match self {
            Self::Float(v) => serde_json::json!(v),
            Self::Vec3(v) => serde_json::json!(v),
            Self::Color(v) => serde_json::json!(v),
            Self::Uint(v) => serde_json::json!(v),
            Self::Bool(v) => serde_json::json!(v),
        }
    }
}
/// A named typed parameter. References use `namespace.name`, or `emitter.name.key`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    /// Namespace that owns this value.
    pub namespace: Namespace,
    /// Stable parameter name; emitter parameters use `emitterName.key`.
    pub name: String,
    /// Inspector tooltip and agent documentation.
    pub doc: String,
    /// Typed initial value.
    pub default: Value,
}
impl Parameter {
    /// Canonical binding address.
    pub fn key(&self) -> String {
        let scope = match self.namespace {
            Namespace::System => "system",
            Namespace::Emitter => "emitter",
            Namespace::Particle => "particle",
            Namespace::User => "user",
        };
        format!("{scope}.{}", self.name)
    }
}
/// A named module instance. Its name preserves identity through stack reordering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackModule {
    /// Unique within its emitter, independent of position or module type.
    pub name: String,
    /// Fully typed module settings.
    pub module: Module,
    /// Module field to parameter address. Values must match that field's exact type.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, String>,
    /// Whether the module runs. A disabled module keeps its name, position, settings
    /// and bindings, but simulation, code generation and rendering skip it.
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub enabled: bool,
}
impl StackModule {
    /// An enabled module with no parameter bindings.
    pub fn new(name: impl Into<String>, module: Module) -> Self {
        Self {
            name: name.into(),
            module,
            bindings: BTreeMap::new(),
            enabled: true,
        }
    }
}
fn enabled() -> bool {
    true
}
fn is_enabled(enabled: &bool) -> bool {
    *enabled
}
/// Processing stages, always evaluated in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Compute emission demand, respecting capacity.
    Spawn,
    /// Initialize new particles, in authored order.
    Initialize,
    /// Advance state then apply modules, in authored order.
    Update,
    /// Select geometry, material domain and shading options.
    Render,
}
/// Ordered module stacks. Moving a module changes evaluation order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Stacks {
    /// Rate/burst emission.
    pub spawn: Vec<StackModule>,
    /// Initial position, velocity and lifetime.
    pub initialize: Vec<StackModule>,
    /// Ordered forces, curves and collisions.
    pub update: Vec<StackModule>,
    /// Geometry, flipbook, lighting and event outputs.
    pub render: Vec<StackModule>,
}
impl Stacks {
    /// Access one stack without coupling editor code to struct field names.
    pub fn get(&self, stage: Stage) -> &[StackModule] {
        match stage {
            Stage::Spawn => &self.spawn,
            Stage::Initialize => &self.initialize,
            Stage::Update => &self.update,
            Stage::Render => &self.render,
        }
    }
    /// Mutate a detached projection; persist through an authoring command.
    pub fn get_mut(&mut self, stage: Stage) -> &mut Vec<StackModule> {
        match stage {
            Stage::Spawn => &mut self.spawn,
            Stage::Initialize => &mut self.initialize,
            Stage::Update => &mut self.update,
            Stage::Render => &mut self.render,
        }
    }
    /// All stacks in execution order.
    pub const STAGES: [Stage; 4] = [
        Stage::Spawn,
        Stage::Initialize,
        Stage::Update,
        Stage::Render,
    ];
}
/// Conservative world-space culling bounds in metres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
    /// Minimum world position.
    pub min: [f32; 3],
    /// Maximum world position.
    pub max: [f32; 3],
}
/// Hard quality ceilings, checked before allocation and at emission/render time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quality {
    /// Maximum live particles across the effect.
    pub particles: u32,
    /// Maximum estimated screen coverage, in full-screen equivalents.
    pub overdraw: f32,
    /// Maximum dynamic lights across this effect.
    pub lights: u32,
    /// Maximum cosmetic events routed per emitter per tick.
    pub events: u32,
}
impl Default for Quality {
    fn default() -> Self {
        Self {
            particles: 1_000_000,
            overdraw: 16.,
            lights: 8,
            events: 4096,
        }
    }
}
/// An emitter owns its capacity and four ordered stacks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Emitter {
    /// Stable human-readable name; used by event targets and graph identity.
    pub name: String,
    /// Requested capacity, partitioned under the effect quality cap.
    pub capacity: u32,
    /// Conservative culling box. Keep it large enough for all forces.
    pub bounds: Bounds,
    /// Ordered spawn, initialize, update and render modules.
    pub stacks: Stacks,
}
/// One authorable effect system. Live runtime state belongs to an ECS component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    /// Friendly effect name, independent of its asset ID.
    pub name: String,
    /// Deterministic seed combined with tick, emitter and emission ordinal.
    pub seed: u32,
    /// Fixed simulation frequency in ticks per second.
    pub hz: u32,
    /// Per-system quality ceilings.
    #[serde(default)]
    pub quality: Quality,
    /// Typed namespaced parameters, in authored order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    /// Emitters in stable authored order.
    pub emitters: Vec<Emitter>,
}
impl Effect {
    /// Read strict ID-free JSON, rejecting duplicate keys and invalid stacks.
    pub fn parse(text: &str) -> Result<Self, Diagnostic> {
        let effect: Self = decode(ara_json::parse(text).map_err(|e| error(e.to_string()))?)?;
        effect.validate()?;
        Ok(effect)
    }
    /// Canonical ID-free authoring text, written by ara-json.
    pub fn text(&self) -> Result<String, Diagnostic> {
        Ok(ara_json::to_canonical_string(&encode(self)?))
    }
    /// Resolve typed instance overrides and bindings into a detached executable effect.
    /// Disabled modules are left out: this is what simulation and code generation run.
    pub fn resolve(&self, overrides: &BTreeMap<String, Value>) -> Result<Self, Diagnostic> {
        let mut values = self
            .parameters
            .iter()
            .map(|p| (p.key(), &p.default))
            .collect::<BTreeMap<_, _>>();
        for (key, value) in overrides {
            let default = values
                .get(key)
                .ok_or_else(|| error(format!("unknown parameter {key}")))?;
            if std::mem::discriminant(*default) != std::mem::discriminant(value) {
                return Err(error(format!("parameter type mismatch: {key}")));
            }
            values.insert(key.clone(), value);
        }
        let mut result = self.clone();
        // Disabled modules stay authored but never execute.
        for emitter in &mut result.emitters {
            for stage in Stacks::STAGES {
                emitter.stacks.get_mut(stage).retain(|item| item.enabled);
            }
        }
        for emitter in &mut result.emitters {
            for stage in Stacks::STAGES {
                for item in emitter.stacks.get_mut(stage) {
                    let mut module = encode(&item.module)?;
                    for (field, key) in &item.bindings {
                        let parameter = values
                            .get(key)
                            .ok_or_else(|| error(format!("unknown binding {key}")))?;
                        if !item.module.accepts_binding(field, parameter) {
                            return Err(error(format!(
                                "binding type mismatch: {field} from {key}"
                            )));
                        }
                        let literal = parameter.literal();
                        let object = module
                            .as_object_mut()
                            .ok_or_else(|| error("module must be object"))?;
                        if field == "op" || !object.contains_key(field) {
                            return Err(error(format!("unknown bindable field {field}")));
                        }
                        let old = &object[field];
                        if old.is_array() != literal.is_array()
                            || old.is_boolean() != literal.is_boolean()
                            || old.as_array().map(Vec::len) != literal.as_array().map(Vec::len)
                        {
                            return Err(error(format!("binding shape mismatch: {field}")));
                        }
                        object.insert(field.clone(), literal);
                    }
                    item.module = decode(module)?;
                    item.bindings.clear();
                }
            }
        }
        result.validate()?;
        Ok(result)
    }
    /// Stable per-emitter partitions under the quality particle cap.
    pub fn capacities(&self) -> Vec<u32> {
        let mut remaining = self.quality.particles;
        self.emitters
            .iter()
            .map(|e| {
                let n = e.capacity.min(remaining);
                remaining -= n;
                n
            })
            .collect()
    }
    /// Content identity for cooked artifacts, excluding graph layout and instance state.
    pub fn content_hash(&self) -> Result<String, Diagnostic> {
        Ok(blake3::hash(self.text()?.as_bytes()).to_hex().to_string())
    }
}
