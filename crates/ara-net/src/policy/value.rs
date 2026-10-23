use super::*;

// Self-describing bounded values avoid JSON strings and postcard's deserialize_any gap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum WireValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Array(Vec<WireValue>),
    Object(BTreeMap<String, WireValue>),
    Reference(crate::NetId, String),
}
impl WireValue {
    pub(super) fn from_json(v: Value, depth: usize) -> Result<Self> {
        if depth > 16 {
            return Err(NetError("wire value nesting exceeds 16".into()));
        }
        Ok(match v {
            Value::Null => Self::Null,
            Value::Bool(v) => Self::Bool(v),
            Value::Number(v) => {
                if let Some(v) = v.as_u64() {
                    Self::UInt(v)
                } else if let Some(v) = v.as_i64() {
                    Self::Int(v)
                } else {
                    Self::Float(
                        v.as_f64()
                            .ok_or_else(|| NetError("non-finite wire number".into()))?,
                    )
                }
            }
            Value::String(v) if v.len() <= 1024 => Self::Text(v),
            Value::Array(v) if v.len() <= 256 => Self::Array(
                v.into_iter()
                    .map(|v| Self::from_json(v, depth + 1))
                    .collect::<Result<_>>()?,
            ),
            Value::Object(v) if v.len() <= 256 => Self::Object(
                v.into_iter()
                    .map(|(k, v)| Ok((k, Self::from_json(v, depth + 1)?)))
                    .collect::<Result<_>>()?,
            ),
            _ => return Err(NetError("wire string/collection bound exceeded".into())),
        })
    }
    pub(super) fn json(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(v) => Value::Bool(*v),
            Self::Int(v) => Value::from(*v),
            Self::UInt(v) => Value::from(*v),
            Self::Float(v) => Value::from(*v),
            Self::Text(v) => Value::String(v.clone()),
            Self::Reference(_, text) => Value::String(text.clone()),
            Self::Array(v) => Value::Array(v.iter().map(Self::json).collect()),
            Self::Object(v) => {
                Value::Object(v.iter().map(|(k, v)| (k.clone(), v.json())).collect())
            }
        }
    }
}

pub(super) fn quantize(value: &Value, precision: Option<f64>, decode: bool) -> Result<Value> {
    let Some(q) = precision else {
        return Ok(value.clone());
    };
    if q == 1.0 && (value.as_i64().is_some() || value.as_u64().is_some()) {
        return Ok(value.clone());
    }
    if let Some(array) = value.as_array() {
        return Ok(Value::Array(
            array
                .iter()
                .map(|v| quantize(v, precision, decode))
                .collect::<Result<_>>()?,
        ));
    }
    let v = value
        .as_f64()
        .ok_or_else(|| NetError("quantization requires numeric fields".into()))?;
    if decode {
        return Ok(Value::from(v * q));
    }
    let scaled = (v / q).round();
    if !scaled.is_finite() || scaled.abs() >= i64::MAX as f64 {
        return Err(NetError("quantized field overflow".into()));
    }
    Ok(Value::from(scaled as i64))
}
#[derive(Clone, Copy)]
pub(super) enum Rotation {
    Quaternion,
    EulerDegrees,
    EulerRadians,
}

pub(super) fn blend(
    a: &WireValue,
    b: &WireValue,
    t: f64,
    rotation: Option<Rotation>,
    precision: f64,
) -> WireValue {
    fn number(v: &WireValue) -> Option<f64> {
        match v {
            WireValue::Int(v) => Some(*v as f64),
            WireValue::UInt(v) => Some(*v as f64),
            WireValue::Float(v) => Some(*v),
            _ => None,
        }
    }
    fn quaternion(
        value: &WireValue,
        rotation: Rotation,
        precision: f64,
    ) -> Option<bevy_math::Quat> {
        use bevy_math::{EulerRot, Quat};
        let WireValue::Array(values) = value else {
            return None;
        };
        match rotation {
            Rotation::Quaternion if values.len() == 4 => {
                let v = [
                    number(&values[0])?,
                    number(&values[1])?,
                    number(&values[2])?,
                    number(&values[3])?,
                ]
                .map(|v| (v * precision) as f32);
                let q = Quat::from_array(v);
                Some(if q.length_squared() > f32::EPSILON {
                    q.normalize()
                } else {
                    Quat::IDENTITY
                })
            }
            Rotation::EulerDegrees | Rotation::EulerRadians if values.len() == 3 => {
                let mut v = [
                    number(&values[0])?,
                    number(&values[1])?,
                    number(&values[2])?,
                ]
                .map(|v| (v * precision) as f32);
                if matches!(rotation, Rotation::EulerDegrees) {
                    v = v.map(f32::to_radians);
                }
                Some(Quat::from_euler(EulerRot::XYZ, v[0], v[1], v[2]))
            }
            _ => None,
        }
    }
    if let Some(rotation) = rotation
        && let (Some(a), Some(b)) = (
            quaternion(a, rotation, precision),
            quaternion(b, rotation, precision),
        )
    {
        let q = a.slerp(b, t as f32);
        let values = match rotation {
            Rotation::Quaternion => q.to_array().into_iter().collect::<Vec<_>>(),
            Rotation::EulerDegrees | Rotation::EulerRadians => {
                let (x, y, z) = q.to_euler(bevy_math::EulerRot::XYZ);
                let v = [x, y, z];
                if matches!(rotation, Rotation::EulerDegrees) {
                    v.map(f32::to_degrees).to_vec()
                } else {
                    v.to_vec()
                }
            }
        };
        return WireValue::Array(
            values
                .into_iter()
                .map(|v| WireValue::Float(f64::from(v) / precision))
                .collect(),
        );
    }
    if let (Some(a), Some(b)) = (number(a), number(b)) {
        return WireValue::Float(a + (b - a) * t);
    }
    if let (WireValue::Array(a), WireValue::Array(b)) = (a, b) {
        return WireValue::Array(
            a.iter()
                .zip(b)
                .map(|(a, b)| blend(a, b, t, None, 1.0))
                .collect(),
        );
    }
    a.clone()
}
