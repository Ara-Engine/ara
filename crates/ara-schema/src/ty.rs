//! Field value types and their text form (`f32`, `vec3`, `list<Slot>`, `asset<prefab>`).

use std::fmt;
use std::str::FromStr;

/// The type of a field's value. Text form in parentheses.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ValueType {
    /// `bool`
    Bool,
    /// `i32`
    I32,
    /// `i64`
    I64,
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `u64`
    U64,
    /// `f32`
    F32,
    /// `f64`
    F64,
    /// `string`
    String,
    /// `vec2`: `[x, y]`
    Vec2,
    /// `vec3`: `[x, y, z]`
    Vec3,
    /// `vec4`: `[x, y, z, w]`
    Vec4,
    /// `quat`: `[x, y, z, w]`, default identity
    Quat,
    /// `color`: linear `[r, g, b, a]`, default opaque white
    Color,
    /// `entity`: an entity in the same document, by record ID or entity address
    /// (`e_8b21`, `e_4f9a/m4tz`); nullable.
    Entity,
    /// `asset` or `asset<kind>`: an `@path#id` reference, optionally restricted to a
    /// document kind; nullable.
    Asset(Option<String>),
    /// `json`: any JSON value.
    Json,
    /// A registered struct or enum, by name (`Slot`).
    Named(String),
    /// `option<T>`: `T` or `null`.
    Option(Box<ValueType>),
    /// `list<T>`: an array replaced as a whole. Never addressed by index.
    List(Box<ValueType>),
    /// `keyed<T>`: an array of structs with an `id` field, addressed as `[id]`.
    Keyed(Box<ValueType>),
    /// `map<T>`: an object with string keys, addressed as `[key]`.
    Map(Box<ValueType>),
}

impl ValueType {
    /// Turns `list<T>` (also inside `option`) into `keyed<T>`. Used by `#[ara(keyed)]`.
    pub fn keyed(self) -> ValueType {
        match self {
            ValueType::List(t) => ValueType::Keyed(t),
            ValueType::Option(t) => ValueType::Option(Box::new(t.keyed())),
            other => other,
        }
    }

    /// Restricts asset references (also inside containers) to one document kind.
    /// Used by `#[ara(asset_kind = "prefab")]`.
    pub fn with_asset_kind(self, kind: &str) -> ValueType {
        let wrap = |t: Box<ValueType>| Box::new(t.with_asset_kind(kind));
        match self {
            ValueType::Asset(_) => ValueType::Asset(Some(kind.to_owned())),
            ValueType::Option(t) => ValueType::Option(wrap(t)),
            ValueType::List(t) => ValueType::List(wrap(t)),
            ValueType::Map(t) => ValueType::Map(wrap(t)),
            other => other,
        }
    }

    /// Number of components for vector-like types.
    pub fn vector_len(&self) -> Option<usize> {
        match self {
            ValueType::Vec2 => Some(2),
            ValueType::Vec3 => Some(3),
            ValueType::Vec4 | ValueType::Quat | ValueType::Color => Some(4),
            _ => None,
        }
    }

    /// Index of a named vector component (`x`, `y`, `z`, `w`; `r`, `g`, `b`, `a` for colors).
    pub fn component_index(&self, name: &str) -> Option<usize> {
        let names: &[&str] = match self {
            ValueType::Color => &["r", "g", "b", "a"],
            _ => &["x", "y", "z", "w"],
        };
        let i = names.iter().position(|n| *n == name)?;
        (i < self.vector_len()?).then_some(i)
    }

    /// True for integer and float types.
    pub fn is_number(&self) -> bool {
        self.int_bounds().is_some() || matches!(self, ValueType::F32 | ValueType::F64)
    }

    /// Inclusive bounds of integer types.
    pub fn int_bounds(&self) -> Option<(i128, i128)> {
        Some(match self {
            ValueType::I32 => (i32::MIN.into(), i32::MAX.into()),
            ValueType::I64 => (i64::MIN.into(), i64::MAX.into()),
            ValueType::U8 => (0, u8::MAX.into()),
            ValueType::U16 => (0, u16::MAX.into()),
            ValueType::U32 => (0, u32::MAX.into()),
            ValueType::U64 => (0, u64::MAX.into()),
            _ => return None,
        })
    }

    /// Every named type this type mentions.
    pub fn named(&self) -> Vec<&str> {
        match self {
            ValueType::Named(n) => vec![n.as_str()],
            ValueType::Option(t) | ValueType::List(t) | ValueType::Keyed(t) | ValueType::Map(t) => {
                t.named()
            }
            _ => Vec::new(),
        }
    }
}

const PRIMITIVES: &[(&str, ValueType)] = &[
    ("bool", ValueType::Bool),
    ("i32", ValueType::I32),
    ("i64", ValueType::I64),
    ("u8", ValueType::U8),
    ("u16", ValueType::U16),
    ("u32", ValueType::U32),
    ("u64", ValueType::U64),
    ("f32", ValueType::F32),
    ("f64", ValueType::F64),
    ("string", ValueType::String),
    ("vec2", ValueType::Vec2),
    ("vec3", ValueType::Vec3),
    ("vec4", ValueType::Vec4),
    ("quat", ValueType::Quat),
    ("color", ValueType::Color),
    ("entity", ValueType::Entity),
    ("json", ValueType::Json),
];

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((name, _)) = PRIMITIVES.iter().find(|(_, t)| t == self) {
            return f.write_str(name);
        }
        match self {
            ValueType::Asset(None) => f.write_str("asset"),
            ValueType::Asset(Some(kind)) => write!(f, "asset<{kind}>"),
            ValueType::Named(n) => f.write_str(n),
            ValueType::Option(t) => write!(f, "option<{t}>"),
            ValueType::List(t) => write!(f, "list<{t}>"),
            ValueType::Keyed(t) => write!(f, "keyed<{t}>"),
            ValueType::Map(t) => write!(f, "map<{t}>"),
            _ => Ok(()),
        }
    }
}

impl FromStr for ValueType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some((_, t)) = PRIMITIVES.iter().find(|(name, _)| *name == s) {
            return Ok(t.clone());
        }
        if s == "asset" {
            return Ok(ValueType::Asset(None));
        }
        if let Some((head, rest)) = s.split_once('<') {
            let inner = rest
                .strip_suffix('>')
                .ok_or_else(|| format!("unclosed `<` in type `{s}`"))?;
            let boxed = || inner.parse().map(Box::new);
            return match head {
                "asset" if crate::grammar::is_kind_name(inner) => {
                    Ok(ValueType::Asset(Some(inner.to_owned())))
                }
                "option" => Ok(ValueType::Option(boxed()?)),
                "list" => Ok(ValueType::List(boxed()?)),
                "keyed" => Ok(ValueType::Keyed(boxed()?)),
                "map" => Ok(ValueType::Map(boxed()?)),
                _ => Err(format!("unknown type `{s}`")),
            };
        }
        if crate::grammar::is_type_name(s) {
            return Ok(ValueType::Named(s.to_owned()));
        }
        Err(format!("unknown type `{s}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::ValueType;

    #[test]
    fn text_form_round_trips() {
        for text in [
            "f32",
            "vec3",
            "asset",
            "asset<prefab>",
            "option<list<Slot>>",
            "keyed<Slot>",
            "map<json>",
        ] {
            let ty: ValueType = text.parse().expect("valid type");
            assert_eq!(ty.to_string(), text);
        }
        assert!("list<f32".parse::<ValueType>().is_err());
        assert!("float".parse::<ValueType>().is_err());
    }
}
