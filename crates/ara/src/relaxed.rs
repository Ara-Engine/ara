//! A forgiving JSON reader for command-line arguments.
//!
//! Shells mangle JSON: Windows PowerShell 5.1 strips the double quotes inside native
//! arguments, so `--args '{"entity":"e_8b21"}'` arrives as `{entity:e_8b21}`. This
//! reader accepts strict JSON plus unquoted keys and bare string values:
//! `{entity: e_8b21, value: [1, 0, 0, 1]}`, `[e_radio1]`, `{why: Fix the crate}`.
//! Bare words that are JSON literals (`3`, `true`, `null`) keep their type.
//! Single-quoted strings (`'a, b'`) keep commas and brackets.

use serde_json::{Map, Value};

/// Parses `text` as relaxed JSON.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Reader {
        s: text.as_bytes(),
        text,
        i: 0,
    };
    let v = p.value(false)?;
    p.ws();
    if p.i < p.s.len() {
        return Err(format!("unexpected `{}` at column {}", p.rest(), p.i + 1));
    }
    Ok(v)
}

struct Reader<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl Reader<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn rest(&self) -> String {
        self.text[self.i..].chars().take(12).collect()
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        let hit = self.s.get(self.i) == Some(&c);
        self.i += usize::from(hit);
        hit
    }

    fn value(&mut self, key: bool) -> Result<Value, String> {
        self.ws();
        match self.s.get(self.i) {
            None => Err("expected a value".into()),
            Some(b'{') if !key => self.object(),
            Some(b'[') if !key => self.array(),
            Some(b'"') => self.quoted(),
            Some(b'\'') => self.single(),
            Some(_) => self.bare(key),
        }
    }

    fn quoted(&mut self) -> Result<Value, String> {
        let start = self.i;
        self.i += 1;
        while let Some(&c) = self.s.get(self.i) {
            self.i += 1;
            match c {
                b'\\' => self.i += 1,
                b'"' => {
                    return serde_json::from_str(&self.text[start..self.i])
                        .map_err(|e| format!("bad string at column {}: {e}", start + 1));
                }
                _ => {}
            }
        }
        Err(format!("unclosed `\"` at column {}", start + 1))
    }

    fn single(&mut self) -> Result<Value, String> {
        let start = self.i + 1;
        let len = self.text[start..]
            .find('\'')
            .ok_or_else(|| format!("unclosed `'` at column {start}"))?;
        self.i = start + len + 1;
        Ok(Value::String(self.text[start..start + len].to_owned()))
    }

    /// A bare word up to `,`, `}` or `]` (and `:` for keys).
    fn bare(&mut self, key: bool) -> Result<Value, String> {
        let start = self.i;
        while let Some(&c) = self.s.get(self.i) {
            if matches!(c, b',' | b'}' | b']') || (key && c == b':') {
                break;
            }
            self.i += 1;
        }
        let word = self.text[start..self.i].trim();
        if word.is_empty() {
            return Err(format!("expected a value at column {}", start + 1));
        }
        Ok(match serde_json::from_str::<Value>(word) {
            Ok(v) if !key && !v.is_object() && !v.is_array() => v,
            _ => Value::String(word.to_owned()),
        })
    }

    fn object(&mut self) -> Result<Value, String> {
        self.i += 1;
        let mut out = Map::new();
        if self.eat(b'}') {
            return Ok(Value::Object(out));
        }
        loop {
            let Value::String(k) = self.value(true)? else {
                return Err("object keys must be text".into());
            };
            if !self.eat(b':') {
                return Err(format!("expected `:` after `{k}`"));
            }
            let v = self.value(false)?;
            out.insert(k, v);
            if self.eat(b'}') {
                return Ok(Value::Object(out));
            }
            if !self.eat(b',') {
                return Err(format!("expected `,` or `}}` at column {}", self.i + 1));
            }
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.i += 1;
        let mut out = Vec::new();
        if self.eat(b']') {
            return Ok(Value::Array(out));
        }
        loop {
            out.push(self.value(false)?);
            if self.eat(b']') {
                return Ok(Value::Array(out));
            }
            if !self.eat(b',') {
                return Err(format!("expected `,` or `]` at column {}", self.i + 1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse;

    #[test]
    fn reads_json_with_its_quotes_stripped() {
        assert_eq!(
            parse(
                "{entity:e_beacon1,component:Light,value:{kind:point,intensity:1500,color:[1,0.214,0,1]}}"
            ),
            Ok(
                json!({ "entity": "e_beacon1", "component": "Light", "value": { "kind": "point", "intensity": 1500, "color": [1, 0.214, 0, 1] } })
            )
        );
        assert_eq!(parse("[e_radio1]"), Ok(json!(["e_radio1"])));
        assert_eq!(
            parse("{ entity: name:Floor/Crate, why: Fix the crate, on: true }"),
            Ok(json!({ "entity": "name:Floor/Crate", "why": "Fix the crate", "on": true }))
        );
        assert_eq!(
            parse(r#"{"a": "x, y", b: 'c, [d]'}"#),
            Ok(json!({ "a": "x, y", "b": "c, [d]" }))
        );
        assert!(parse("{a: 1").is_err() && parse("a, b").is_err() && parse("").is_err());
    }
}
