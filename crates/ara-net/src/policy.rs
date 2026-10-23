mod value;
use crate::{NetError, Result};
use ara_schema::{Audience, Interpolate, Replication, SchemaDb, Unit, ValueType};
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use value::{Rotation, WireValue, blend, quantize};

/// Stable field identity derived from the qualified schema path; collisions are rejected.
pub type FieldId = u64;

/// Encoded field payload accounting, excluding packet/ACK/encryption overhead.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldBytes {
    /// Bytes offered to replication when this field changed.
    pub bytes: u64,
    /// Number of changed field values offered.
    pub updates: u64,
}

/// One recipient-safe field projection. Authority-only fields are absent before encoding.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Projection(pub(crate) BTreeMap<FieldId, WireValue>);
impl Projection {
    pub(crate) fn references(&self) -> Vec<crate::NetId> {
        fn visit(value: &WireValue, out: &mut Vec<crate::NetId>) {
            match value {
                WireValue::Reference(id, _) => out.push(*id),
                WireValue::Array(v) => {
                    for v in v {
                        visit(v, out)
                    }
                }
                WireValue::Object(v) => {
                    for v in v.values() {
                        visit(v, out)
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for v in self.0.values() {
            visit(v, &mut out);
        }
        out
    }
}

#[derive(Clone)]
struct Field {
    component: String,
    path: Vec<String>,
    ty: ValueType,
    policy: Replication,
    unit: Option<Unit>,
}

/// Compiled schema paths/codecs shared by authority, clients and field-level tooling.
#[derive(Clone)]
pub struct PolicyRegistry {
    schema: SchemaDb,
    fields: BTreeMap<FieldId, Field>,
    hash: [u8; 32],
}
impl PolicyRegistry {
    /// Compiles reflected component fields, checking IDs and bounded policy support.
    pub fn new(schema: SchemaDb) -> Result<Self> {
        if let Some(d) = schema.verify().into_iter().find(|d| d.is_error()) {
            return Err(NetError(d.message));
        }
        let hash = ara_sim::StateHash::of_bytes(
            ara_json::to_canonical_string(&ara_json::sort_keys(&schema.reflection())).as_bytes(),
        )
        .0;
        let mut this = Self {
            schema,
            fields: BTreeMap::new(),
            hash,
        };
        let components: Vec<_> = this.schema.components().cloned().collect();
        for component in components {
            for field in component.fields() {
                this.compile(
                    &component.name,
                    vec![field.name.clone()],
                    &field.ty,
                    field.replicate,
                    0,
                    field.unit,
                )?;
            }
        }
        Ok(this)
    }
    fn compile(
        &mut self,
        component: &str,
        path: Vec<String>,
        ty: &ValueType,
        policy: Replication,
        depth: usize,
        unit: Option<Unit>,
    ) -> Result<()> {
        if depth > 16 {
            return Err(NetError("recursive network schema".into()));
        }
        if let Some(q) = policy.quantize
            && (!q.is_finite() || q <= 0.0)
        {
            return Err(NetError("quantization must be finite and positive".into()));
        }
        if let ValueType::Named(name) = ty {
            let desc = self
                .schema
                .get(name)
                .ok_or_else(|| NetError(format!("unknown schema {name}")))?
                .clone();
            if !desc.fields().is_empty() {
                let scoped = desc
                    .fields()
                    .iter()
                    .any(|f| f.replicate != Replication::default());
                for field in desc.fields() {
                    let mut child = path.clone();
                    child.push(field.name.clone());
                    // Plain value types inherit the containing field. Once a value
                    // declares child policies, unannotated children stay private.
                    let mut scope = if scoped { field.replicate } else { policy };
                    scope.rollback |= policy.rollback;
                    if policy.audience == Audience::None {
                        scope.audience = Audience::None;
                    }
                    if policy.audience == Audience::Owner && scope.audience == Audience::Relevant {
                        scope.audience = Audience::Owner;
                    }
                    self.compile(
                        component,
                        child,
                        &field.ty,
                        scope,
                        depth + 1,
                        field.unit.or(unit),
                    )?;
                }
                return Ok(());
            }
        }
        if matches!(
            ty,
            ValueType::List(_) | ValueType::Map(_) | ValueType::Keyed(_) | ValueType::Option(_)
        ) {
            // Scoped fields inside collections require per-element paths, not an opaque blob.
            let mut scoped = false;
            self.check_nested(ty, 0, &mut scoped)?;
            if scoped {
                return Err(NetError(format!(
                    "{component}.{}: scoped collection elements require a separate replicated entity",
                    path.join(".")
                )));
            }
        }
        let name = format!("{component}.{}", path.join("."));
        let id = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        if self
            .fields
            .insert(
                id,
                Field {
                    component: component.into(),
                    path,
                    ty: ty.clone(),
                    policy,
                    unit,
                },
            )
            .is_some()
        {
            return Err(NetError(format!("field ID collision at {name}")));
        }
        Ok(())
    }
    fn check_nested(&self, ty: &ValueType, depth: usize, scoped: &mut bool) -> Result<()> {
        if depth > 16 {
            return Err(NetError("recursive wire collection".into()));
        }
        match ty {
            ValueType::List(t) | ValueType::Map(t) | ValueType::Keyed(t) | ValueType::Option(t) => {
                self.check_nested(t, depth + 1, scoped)?
            }
            ValueType::Named(n) => {
                if let Some(t) = self.schema.get(n) {
                    for f in t.fields() {
                        *scoped |= f.replicate != Replication::default();
                        self.check_nested(&f.ty, depth + 1, scoped)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    /// Negotiated compatibility hash, including schema policies and versions.
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }
    /// Reflected schema used by this protocol.
    pub fn schema(&self) -> &SchemaDb {
        &self.schema
    }
    /// Iterates diagnostic field names in stable ID order.
    pub fn names(&self) -> impl Iterator<Item = (FieldId, String)> + '_ {
        self.fields
            .iter()
            .map(|(&id, f)| (id, format!("{}.{}", f.component, f.path.join("."))))
    }
    pub(crate) fn validate_feature(&self, component: &str) -> Result<()> {
        let mut count = 0;
        for f in self.fields.values().filter(|f| f.component == component) {
            count += 1;
            if f.policy.audience != Audience::Relevant || !f.policy.rollback {
                return Err(NetError(format!(
                    "{component}: native prediction needs complete relevant rollback fields; use scoped components for other state"
                )));
            }
        }
        if count == 0 {
            return Err(NetError("feature has no schema state".into()));
        }
        Ok(())
    }
    pub(crate) fn has_quantization(&self, component: &str) -> bool {
        self.fields
            .values()
            .any(|f| f.component == component && f.policy.quantize.is_some())
    }
    /// Maps schema entity references to network lifetimes before serialization.
    /// Reference text is retained for gameplay/editor diagnostics.
    pub fn bind_references(
        &self,
        projection: &mut Projection,
        resolve: impl Fn(&str) -> Option<crate::NetId>,
    ) -> Result<()> {
        fn bind(
            value: &mut WireValue,
            ty: &ValueType,
            resolve: &impl Fn(&str) -> Option<crate::NetId>,
        ) -> Result<()> {
            match ty {
                ValueType::Entity => {
                    if let WireValue::Text(text) = value {
                        let id = resolve(text).ok_or_else(|| {
                            NetError(format!("unresolved authoritative entity reference {text}"))
                        })?;
                        *value = WireValue::Reference(id, std::mem::take(text));
                    }
                }
                ValueType::List(t) => {
                    if let WireValue::Array(v) = value {
                        for v in v {
                            bind(v, t, resolve)?
                        }
                    }
                }
                ValueType::Map(t) | ValueType::Keyed(t) => {
                    if let WireValue::Object(v) = value {
                        for v in v.values_mut() {
                            bind(v, t, resolve)?
                        }
                    }
                }
                ValueType::Option(t) if *value != WireValue::Null => bind(value, t, resolve)?,
                _ => {}
            }
            Ok(())
        }
        for (id, value) in &mut projection.0 {
            let f = self
                .fields
                .get(id)
                .ok_or_else(|| NetError("unknown reference field".into()))?;
            bind(value, &f.ty, &resolve)?;
        }
        Ok(())
    }
    /// Projects one component to public or owner-only wire state.
    pub fn project(
        &self,
        component: &str,
        value: &Value,
        audience: Audience,
    ) -> Result<Projection> {
        let mut result = BTreeMap::new();
        for (&id, field) in &self.fields {
            if field.component != component
                || field.policy.audience != audience
                || audience == Audience::None
            {
                continue;
            }
            let mut v = value;
            for key in &field.path {
                v = v.get(key).ok_or_else(|| {
                    NetError(format!("unmaterialized runtime field {component}.{key}"))
                })?;
            }
            result.insert(
                id,
                WireValue::from_json(quantize(v, field.policy.quantize, false)?, 0)?,
            );
        }
        Ok(Projection(result))
    }
    /// Decodes a scoped component, preserving absence of fields the recipient cannot see.
    pub fn component(&self, projection: &Projection, component: &str) -> Result<Value> {
        let mut result = serde_json::Map::new();
        for (&id, wire) in &projection.0 {
            let field = self
                .fields
                .get(&id)
                .ok_or_else(|| NetError("unknown field ID".into()))?;
            if field.component != component {
                continue;
            }
            let mut value = quantize(&wire.json(), field.policy.quantize, true)?;
            if field.policy.quantize.is_some() {
                if matches!(field.ty, ValueType::I32 | ValueType::I64)
                    && let Some(number) = value.as_f64()
                {
                    value = Value::from(number.round() as i64);
                }
                if matches!(
                    field.ty,
                    ValueType::U8 | ValueType::U16 | ValueType::U32 | ValueType::U64
                ) && let Some(number) = value.as_f64()
                {
                    value = Value::from(number.round() as u64);
                }
            }
            set_path(&mut result, &field.path, value)?;
        }
        Ok(Value::Object(result))
    }
    /// Interpolates visible fields; discrete/new fields snap to the latest baseline.
    pub fn interpolate(&self, a: &Projection, b: &Projection, alpha: f32) -> Projection {
        let mut result = b.clone();
        for (&id, value) in &a.0 {
            if let (Some(field), Some(next)) = (self.fields.get(&id), b.0.get(&id))
                && field.policy.interpolate != Interpolate::None
            {
                result.0.insert(
                    id,
                    blend(
                        value,
                        next,
                        f64::from(if alpha.is_finite() {
                            alpha.clamp(0.0, 1.0)
                        } else {
                            0.0
                        }),
                        if field.policy.interpolate == Interpolate::Slerp {
                            match field.ty {
                                ValueType::Quat => Some(Rotation::Quaternion),
                                ValueType::Vec3 => Some(if field.unit == Some(Unit::Radians) {
                                    Rotation::EulerRadians
                                } else {
                                    Rotation::EulerDegrees
                                }),
                                _ => None,
                            }
                        } else {
                            None
                        },
                        field.policy.quantize.unwrap_or(1.0),
                    ),
                );
            }
        }
        result
    }
    /// Counts actual changed-field encoded payload sizes; headers remain transport stats.
    pub fn account(
        &self,
        previous: &Projection,
        next: &Projection,
        out: &mut BTreeMap<String, FieldBytes>,
    ) -> Result<()> {
        if previous == next {
            return Ok(());
        }
        for (&id, value) in &next.0 {
            let field = self
                .fields
                .get(&id)
                .ok_or_else(|| NetError("unregistered profile field".into()))?;
            let entry = out
                .entry(format!("{}.{}", field.component, field.path.join(".")))
                .or_default();
            entry.bytes += postcard::to_allocvec(&(id, value))?.len() as u64;
            entry.updates += 1;
        }
        Ok(())
    }
}
pub(crate) fn set_path(
    out: &mut serde_json::Map<String, Value>,
    path: &[String],
    value: Value,
) -> Result<()> {
    let Some((first, rest)) = path.split_first() else {
        return Err(NetError("empty field path".into()));
    };
    if rest.is_empty() {
        out.insert(first.clone(), value);
    } else {
        let child = out
            .entry(first.clone())
            .or_insert_with(|| Value::Object(Default::default()));
        set_path(
            child
                .as_object_mut()
                .ok_or_else(|| NetError("overlapping schema paths".into()))?,
            rest,
            value,
        )?;
    }
    Ok(())
}
