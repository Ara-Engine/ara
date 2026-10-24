//! Type, field and document-kind descriptors: the data the Schema DB stores.
//!
//! Rust types produce these through `#[derive(Schema)]`; data-only types are parsed
//! from JSON (see [`crate::dynamic`]). Both end up identical inside the DB.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::ValueType;

/// Upgrades a stored value from version `from` to `from + 1`, in place.
///
/// The reader calls it once per version step, oldest first.
pub type UpgradeFn = fn(from: u32, value: &mut Value) -> Result<(), String>;

/// One registered type: a component, document record, command params, value struct
/// or enum.
#[derive(Debug, Clone)]
pub struct TypeDesc {
    /// Stable identity, PascalCase (`Transform`). Never reused for another type.
    pub name: String,
    /// Required documentation. Feeds tooltips, docs, JSON Schema and agent tools.
    pub doc: String,
    /// Owning module (`core`, `game`); informational.
    pub module: String,
    /// Data version, starting at 1. Bump it when stored data needs an upgrade.
    pub version: u32,
    /// Former names. Files using them are renamed on read.
    pub aliases: Vec<String>,
    /// Deprecation message, if the type should no longer be used.
    pub deprecated: Option<String>,
    /// Fields or variants.
    pub shape: Shape,
    /// Upgrade hook for data stored with an older `version`.
    pub upgrade: Option<UpgradeFn>,
}

/// What a type is made of.
#[derive(Debug, Clone)]
pub enum Shape {
    /// A struct with named fields, in reflection order.
    Struct {
        /// What the struct is used as.
        role: StructRole,
        /// Fields in reflection (and canonical file) order.
        fields: Vec<FieldDesc>,
    },
    /// A unit-variant enum stored as its variant name. The first variant is the default.
    Enum {
        /// Variants in declaration order.
        variants: Vec<VariantDesc>,
    },
}

/// The role of a struct type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructRole {
    /// Attaches to entities; stored inline in entity records under its type name.
    Component,
    /// A document record type; records are keyed by IDs with this prefix.
    Record(RecordInfo),
    /// Parameters of a command, query or job.
    Params,
    /// A plain value nested inside other types.
    Value,
    /// A world singleton (a simulation resource such as game rules), not attached to
    /// entities.
    Resource,
}

/// Extra data for record types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordInfo {
    /// ID prefix (`e` for entities, giving IDs like `e_8b21`).
    pub prefix: String,
    /// Whether records also carry components as PascalCase keys.
    pub components: bool,
}

impl TypeDesc {
    /// The fields of a struct type; empty for enums.
    pub fn fields(&self) -> &[FieldDesc] {
        match &self.shape {
            Shape::Struct { fields, .. } => fields,
            Shape::Enum { .. } => &[],
        }
    }

    /// Looks up a field by its current name.
    pub fn field(&self, name: &str) -> Option<&FieldDesc> {
        self.fields().iter().find(|f| f.name == name)
    }

    /// The variants of an enum type; empty for structs.
    pub fn variants(&self) -> &[VariantDesc] {
        match &self.shape {
            Shape::Enum { variants } => variants,
            Shape::Struct { .. } => &[],
        }
    }

    /// The struct role, or `None` for enums.
    pub fn role(&self) -> Option<&StructRole> {
        match &self.shape {
            Shape::Struct { role, .. } => Some(role),
            Shape::Enum { .. } => None,
        }
    }

    /// True for component types.
    pub fn is_component(&self) -> bool {
        self.role() == Some(&StructRole::Component)
    }

    /// Record metadata, for record types.
    pub fn record_info(&self) -> Option<&RecordInfo> {
        match self.role() {
            Some(StructRole::Record(info)) => Some(info),
            _ => None,
        }
    }

    /// The kind name used in `reflection.json`.
    pub fn kind_name(&self) -> &'static str {
        match &self.shape {
            Shape::Enum { .. } => "enum",
            Shape::Struct { role, .. } => match role {
                StructRole::Component => "component",
                StructRole::Record(_) => "record",
                StructRole::Params => "params",
                StructRole::Value => "struct",
                StructRole::Resource => "resource",
            },
        }
    }
}

/// One field of a struct type.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDesc {
    /// Current name, camelCase (`maxSpeed`).
    pub name: String,
    /// Value type.
    pub ty: ValueType,
    /// Required documentation.
    pub doc: String,
    /// Explicit default. `None` means the type's zero value (see `SchemaDb::field_default`).
    pub default: Option<Value>,
    /// Must be present. Only params fields can be required; component fields always
    /// have defaults.
    pub required: bool,
    /// Physical unit of the number(s).
    pub unit: Option<Unit>,
    /// Inclusive numeric bounds, applied to every component of vectors.
    pub range: Option<Range>,
    /// Example values, shown to agents and in docs.
    pub examples: Vec<Value>,
    /// Type version that introduced the field.
    pub since: Option<u32>,
    /// Deprecation message.
    pub deprecated: Option<String>,
    /// Former names. Files using them are renamed on read.
    pub aliases: Vec<String>,
    /// Network replication policy.
    pub replicate: Replication,
    /// Inspector hints.
    pub editor: EditorHints,
}

impl FieldDesc {
    /// A field with only its required parts set.
    pub fn new(name: impl Into<String>, ty: ValueType, doc: impl Into<String>) -> Self {
        FieldDesc {
            name: name.into(),
            ty,
            doc: doc.into(),
            default: None,
            required: false,
            unit: None,
            range: None,
            examples: Vec::new(),
            since: None,
            deprecated: None,
            aliases: Vec::new(),
            replicate: Replication::default(),
            editor: EditorHints::default(),
        }
    }
}

/// One variant of an enum type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantDesc {
    /// Stored name, camelCase (`spotLight`).
    pub name: String,
    /// Required documentation.
    pub doc: String,
    /// Former names.
    pub aliases: Vec<String>,
    /// Deprecation message.
    pub deprecated: Option<String>,
}

/// Inclusive numeric bounds; either side may be open.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Range {
    /// Smallest allowed value.
    pub min: Option<f64>,
    /// Largest allowed value.
    pub max: Option<f64>,
}

impl Range {
    /// True if `v` lies within the bounds.
    pub fn contains(&self, v: f64) -> bool {
        self.min.is_none_or(|m| v >= m) && self.max.is_none_or(|m| v <= m)
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = |b: Option<f64>| b.map_or("∞".to_owned(), ara_json::format_f64);
        write!(f, "[{}, {}]", side(self.min), side(self.max))
    }
}

macro_rules! units {
    ($($(#[doc = $doc:literal])+ $variant:ident = $symbol:literal,)*) => {
        /// Physical unit of a numeric field, stored as its symbol (`"m"`).
        ///
        /// Units are part of the schema, so tools convert and label values instead of
        /// guessing from docs.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Unit {
            $($(#[doc = $doc])+ $variant,)*
        }

        impl Unit {
            /// Every unit, in declaration order.
            pub const ALL: &[Unit] = &[$(Unit::$variant),*];

            /// The stored symbol (`"m"`, `"deg"`, `"m/s"`).
            pub fn symbol(self) -> &'static str {
                match self {
                    $(Unit::$variant => $symbol,)*
                }
            }
        }
    };
}

units! {
    /// Meters.
    Meters = "m",
    /// Centimeters.
    Centimeters = "cm",
    /// Kilometers.
    Kilometers = "km",
    /// Degrees of angle.
    Degrees = "deg",
    /// Radians.
    Radians = "rad",
    /// Seconds.
    Seconds = "s",
    /// Milliseconds.
    Milliseconds = "ms",
    /// Kilograms.
    Kilograms = "kg",
    /// Meters per second.
    MetersPerSecond = "m/s",
    /// Meters per second squared.
    MetersPerSecondSquared = "m/s2",
    /// Degrees per second.
    DegreesPerSecond = "deg/s",
    /// Radians per second.
    RadiansPerSecond = "rad/s",
    /// Newtons.
    Newtons = "N",
    /// Newton-meters (torque).
    NewtonMeters = "N*m",
    /// Hertz.
    Hertz = "Hz",
    /// Screen pixels.
    Pixels = "px",
    /// Percent (0–100).
    Percent = "%",
    /// Kelvin (color temperature).
    Kelvin = "K",
    /// Lumens.
    Lumens = "lm",
    /// Lux.
    Lux = "lx",
    /// Candela.
    Candela = "cd",
    /// Decibels.
    Decibels = "dB",
}

impl FromStr for Unit {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Unit::ALL
            .iter()
            .copied()
            .find(|u| u.symbol() == s)
            .ok_or_else(|| format!("unknown unit `{s}`"))
    }
}

impl Serialize for Unit {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.symbol())
    }
}

impl<'de> Deserialize<'de> for Unit {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// How a field replicates over the network. The default replicates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Replication {
    /// Which clients receive the field.
    #[serde(default, skip_serializing_if = "is_default")]
    pub audience: Audience,
    /// Wire precision in the field's unit (`0.001` m); `None` sends full precision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantize: Option<f64>,
    /// How remote views smooth between snapshots.
    #[serde(default, skip_serializing_if = "is_default")]
    pub interpolate: Interpolate,
    /// Whether the field is part of the rollback (prediction) state.
    #[serde(default, skip_serializing_if = "is_default")]
    pub rollback: bool,
}

/// Recipients of a replicated field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Audience {
    /// Not replicated; the authority keeps it.
    #[default]
    None,
    /// Every client the entity is relevant to.
    Relevant,
    /// Only the owning client (private state such as an inventory).
    Owner,
}

/// Interpolation of a replicated field on remote views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Interpolate {
    /// Snap to each snapshot.
    #[default]
    None,
    /// Linear interpolation.
    Linear,
    /// Spherical interpolation, for rotations.
    Slerp,
}

/// Inspector hints. They change presentation, never validity.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorHints {
    /// Inspector group heading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Show a slider (needs a range).
    #[serde(default, skip_serializing_if = "is_default")]
    pub slider: bool,
    /// Drag and slider step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// Multi-line text box for strings.
    #[serde(default, skip_serializing_if = "is_default")]
    pub multiline: bool,
    /// Hide from the inspector.
    #[serde(default, skip_serializing_if = "is_default")]
    pub hidden: bool,
    /// Show but do not allow edits.
    #[serde(default, skip_serializing_if = "is_default")]
    pub read_only: bool,
    /// Edit a `vec3`/`vec4` of linear RGB(A) with a colour swatch and picker.
    #[serde(default, skip_serializing_if = "is_default")]
    pub color: bool,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A document kind (scene, prefab): its `$schema`, ID prefix and record types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocKindDesc {
    /// Kind name, lowercase (`scene`).
    pub name: String,
    /// Format version; part of `$schema`.
    pub version: u32,
    /// Required documentation.
    pub doc: String,
    /// Prefix of the document's asset ID (`sc` gives `sc_7kq2m9ab`).
    pub id_prefix: String,
    /// File extension without the dot (`scene`).
    pub extension: String,
    /// Record types the document may contain, by name.
    pub records: Vec<String>,
    /// Whether the document must have exactly one root record (prefabs).
    pub single_root: bool,
}

impl DocKindDesc {
    /// The `$schema` value of documents of this kind: `ara://schema/<name>/<version>`.
    pub fn schema_uri(&self) -> String {
        format!("ara://schema/{}/{}", self.name, self.version)
    }
}
