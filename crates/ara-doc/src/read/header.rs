//! Strict header indexing without constructing or cloning a document's value tree.
use std::{borrow::Cow, fmt};

use ara_schema::SchemaDb;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};

use crate::AssetId;

mod keys;
use keys::Keys;

/// Reads kind and asset ID for indexing. The complete input is still checked for
/// strict JSON syntax and duplicate keys, including inside ignored payloads.
pub fn read_header(text: &str, db: &SchemaDb) -> Option<(String, AssetId)> {
    let mut decoder = serde_json::Deserializer::from_str(text);
    let header = Header::deserialize(&mut decoder).ok()?;
    decoder.end().ok()?;
    let (kind, _) = db.kind_for_schema(header.schema.as_deref()?).ok()?;
    Some((
        kind.name.clone(),
        AssetId::parse(header.id.as_deref()?).ok()?,
    ))
}

struct Header<'a> {
    schema: Option<Cow<'a, str>>,
    id: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Key<'a>(#[serde(borrow)] Cow<'a, str>);

impl<'de> Deserialize<'de> for Header<'de> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct HeaderVisitor;
        impl<'de> Visitor<'de> for HeaderVisitor {
            type Value = Header<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a document object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut header = Header {
                    schema: None,
                    id: None,
                };
                let mut keys = Keys::default();
                while let Some(Key(key)) = map.next_key::<Key<'de>>()? {
                    let field = match key.as_ref() {
                        "$schema" => 1,
                        "id" => 2,
                        _ => 0,
                    };
                    if !keys.insert(key) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    match field {
                        1 => header.schema = map.next_value::<JsonValue<'de, true>>()?.0,
                        2 => header.id = map.next_value::<JsonValue<'de, true>>()?.0,
                        _ => {
                            map.next_value::<JsonValue<'de, false>>()?;
                        }
                    }
                }
                Ok(header)
            }
        }
        decoder.deserialize_map(HeaderVisitor)
    }
}

// Serde's IgnoredAny skips duplicate-key validation. This visitor drains values
// without retaining them, while checking every object with borrowed JSON keys.
struct JsonValue<'a, const KEEP_STRING: bool>(Option<Cow<'a, str>>);

impl<'de, const KEEP: bool> Deserialize<'de> for JsonValue<'de, KEEP> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_any(ValueVisitor::<KEEP>).map(Self)
    }
}

struct ValueVisitor<const KEEP_STRING: bool>;

impl<'de, const KEEP: bool> Visitor<'de> for ValueVisitor<KEEP> {
    type Value = Option<Cow<'de, str>>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
        Ok(if KEEP {
            Some(Cow::Borrowed(value))
        } else {
            None
        })
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(if KEEP {
            Some(Cow::Owned(value.to_owned()))
        } else {
            None
        })
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        Ok(if KEEP { Some(Cow::Owned(value)) } else { None })
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        while sequence.next_element::<JsonValue<'de, false>>()?.is_some() {}
        Ok(None)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut keys = Keys::default();
        while let Some(Key(key)) = map.next_key::<Key<'de>>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate key"));
            }
            map.next_value::<JsonValue<'de, false>>()?;
        }
        Ok(None)
    }
}
