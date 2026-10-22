//! Strict JSON in, canonical JSON out.
//!
//! [`parse`] reads RFC 8259 JSON and rejects duplicate keys, reporting line and
//! column. [`to_canonical_string`] writes the one layout every Ara text file uses:
//!
//! - keys in the map's insertion order (callers insert them in reflection order);
//! - 2-space indentation, LF line endings and a trailing newline;
//! - arrays of numbers, booleans and nulls on one line (`[0, 1, 0]`); every other
//!   non-empty array or object one item per line;
//! - numbers in their shortest round-trip form, integral floats without `.0`.
//!
//! Writing what [`parse`] read from canonical text gives back identical bytes.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
pub use serde_json::{Map, Number, Value};

/// A JSON syntax error, or a duplicate key, with its position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// What went wrong, without the position.
    pub message: String,
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
    /// True when the input is valid JSON except for a repeated object key.
    pub duplicate_key: bool,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {} column {}",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for ParseError {}

const DUPLICATE: &str = "duplicate key";

/// Parses strict JSON, rejecting duplicate object keys.
pub fn parse(text: &str) -> Result<Value, ParseError> {
    serde_json::from_str::<Strict>(text)
        .map(|s| s.0)
        .map_err(|e| {
            let full = e.to_string();
            let message = match full.rfind(" at line ") {
                Some(i) => full[..i].to_owned(),
                None => full,
            };
            ParseError {
                duplicate_key: message.starts_with(DUPLICATE),
                message,
                line: u32::try_from(e.line()).unwrap_or(u32::MAX),
                column: u32::try_from(e.column()).unwrap_or(u32::MAX),
            }
        })
}

/// A [`Value`] whose deserializer fails on duplicate keys.
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StrictVisitor).map(Strict)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Number::from_f64(v).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(Strict(v)) = seq.next_element()? {
            items.push(v);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut obj = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if obj.contains_key(&key) {
                return Err(de::Error::custom(format!("{DUPLICATE} `{key}`")));
            }
            let Strict(v) = map.next_value()?;
            obj.insert(key, v);
        }
        Ok(Value::Object(obj))
    }
}

/// Writes `value` in the canonical layout (see the crate docs), ending with `\n`.
pub fn to_canonical_string(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out.push('\n');
    out
}

fn write_value(out: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&format_number(n)),
        Value::String(s) => write_str(out, s),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) if items.iter().all(is_inline_scalar) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, item, depth);
            }
            out.push(']');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                out.push_str(if i > 0 { ",\n" } else { "\n" });
                indent(out, depth + 1);
                write_value(out, item, depth + 1);
            }
            out.push('\n');
            indent(out, depth);
            out.push(']');
        }
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                out.push_str(if i > 0 { ",\n" } else { "\n" });
                indent(out, depth + 1);
                write_str(out, key);
                out.push_str(": ");
                write_value(out, item, depth + 1);
            }
            out.push('\n');
            indent(out, depth);
            out.push('}');
        }
    }
}

fn is_inline_scalar(v: &Value) -> bool {
    matches!(v, Value::Null | Value::Bool(_) | Value::Number(_))
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Formats a JSON number canonically.
pub fn format_number(n: &Number) -> String {
    if let Some(u) = n.as_u64() {
        u.to_string()
    } else if let Some(i) = n.as_i64() {
        i.to_string()
    } else {
        format_f64(n.as_f64().unwrap_or(0.0))
    }
}

/// Largest magnitude below which every integral `f64` is exact.
const EXACT_INT: f64 = 9_007_199_254_740_992.0;

/// Formats a finite float in its shortest round-trip form. Integral values below
/// 2^53 print as integers (`1`, not `1.0`), and `-0` prints as `0`.
pub fn format_f64(f: f64) -> String {
    if f == 0.0 {
        return "0".to_owned();
    }
    if f.fract() == 0.0 && f.abs() < EXACT_INT {
        // Exact: integral and within the range where f64 represents every integer.
        return format!("{}", f as i64);
    }
    ryu::Buffer::new().format_finite(f).to_owned()
}

/// JSON equality that treats `1`, `1.0` and `1e0` as equal and ignores key order.
pub fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => number_eq(x, y),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

fn number_eq(x: &Number, y: &Number) -> bool {
    let int = |n: &Number| {
        n.as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
    };
    match (int(x), int(y)) {
        (Some(a), Some(b)) => a == b,
        _ => x.as_f64() == y.as_f64(),
    }
}

/// Returns a copy of `value` with every object's keys sorted, for values that have
/// no schema-defined order.
pub fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), sort_keys(&map[k])))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn rejects_duplicate_keys_with_position() {
        let err = parse("{\n  \"a\": 1,\n  \"a\": 2\n}").expect_err("duplicate key");
        assert!(err.duplicate_key);
        assert_eq!(err.message, "duplicate key `a`");
        assert_eq!(err.line, 3);
    }

    #[test]
    fn reports_syntax_position() {
        let err = parse("{\n  \"a\": 1,\n}").expect_err("trailing comma");
        assert!(!err.duplicate_key);
        assert_eq!((err.line, err.column), (3, 1));
    }

    #[test]
    fn numbers_are_shortest_round_trip() {
        let cases = [
            (1.0, "1"),
            (-0.0, "0"),
            (0.1, "0.1"),
            (-2.5, "-2.5"),
            (1e21, "1e21"),
            (1.5e-7, "1.5e-7"),
            (0.30000000000000004, "0.30000000000000004"),
        ];
        for (f, text) in cases {
            assert_eq!(format_f64(f), text);
        }
    }

    #[test]
    fn canonical_layout() {
        let v = parse(r#"{"b":[0,1.0,0],"a":{"x":[{"k":"v"}],"e":[],"o":{}},"s":["q\"\n"]}"#)
            .expect("valid");
        assert_eq!(
            to_canonical_string(&v),
            "{\n  \"b\": [0, 1, 0],\n  \"a\": {\n    \"x\": [\n      {\n        \"k\": \"v\"\n      }\n    ],\n    \"e\": [],\n    \"o\": {}\n  },\n  \"s\": [\n    \"q\\\"\\n\"\n  ]\n}\n"
        );
    }

    #[test]
    fn json_eq_is_numeric_and_order_free() {
        let a = parse(r#"{"x": 1, "y": [2.0]}"#).expect("valid");
        let b = parse(r#"{"y": [2], "x": 1.0}"#).expect("valid");
        assert!(json_eq(&a, &b));
        assert!(!json_eq(&a, &parse(r#"{"x": 1}"#).expect("valid")));
    }

    proptest! {
        #[test]
        fn floats_round_trip_through_text(f in any::<f64>().prop_filter("finite", |f| f.is_finite())) {
            let text = format_f64(f);
            let back: f64 = text.parse().expect("canonical numbers parse");
            prop_assert_eq!(back, if f == 0.0 { 0.0 } else { f });
        }

        #[test]
        fn canonical_text_is_a_fixed_point(v in arb_json()) {
            let once = to_canonical_string(&v);
            let reparsed = parse(&once).expect("canonical output parses");
            prop_assert_eq!(to_canonical_string(&reparsed), once);
        }
    }

    fn arb_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            any::<f64>()
                .prop_filter("finite", |f| f.is_finite())
                .prop_map(Value::from),
            "[a-z\"\\\\\n ]{0,6}".prop_map(Value::String),
        ];
        leaf.prop_recursive(3, 24, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                prop::collection::btree_map("[a-z]{1,3}", inner, 0..4)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        })
    }
}
