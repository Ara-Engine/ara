//! Entity queries: the compact syntax (`Light name:Crate* Light.intensity>500`), its
//! JSON form ([`Filter`]) and evaluation over a [`Tree`].

use ara_diag::{Diagnostic, FixAction, codes, did_you_mean};
use ara_doc::prefab::ResolvedEntity;
use ara_schema::grammar::is_entity_address;
use ara_schema::{FieldPath, SchemaDb, path};
use serde_json::Value;

use crate::params::{Compare, CompareOp, Filter};
use crate::tree::Tree;

const KEYS: [&str; 7] = ["name", "parent", "under", "prefab", "id", "has", "lacks"];
const OPS: [(&str, CompareOp); 6] = [
    (">=", CompareOp::Ge),
    ("<=", CompareOp::Le),
    ("!=", CompareOp::Ne),
    ("=", CompareOp::Eq),
    (">", CompareOp::Gt),
    ("<", CompareOp::Lt),
];

fn invalid(q: &str, msg: String) -> Diagnostic {
    Diagnostic::error(codes::INVALID_QUERY, format!("query `{q}`: {msg}")).path("q")
}

/// Splits on whitespace, keeping `"quoted values"` together (quotes removed).
fn tokens(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in q.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A JSON literal if the text is one, else the text as a string.
fn literal(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

fn is_pascal(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_uppercase()) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Parses the compact syntax into a filter, checking component names and field paths
/// against the Schema DB.
pub fn parse(q: &str, db: &SchemaDb) -> Result<Filter, Diagnostic> {
    let mut f = Filter::default();
    for tok in tokens(q) {
        let negated = tok
            .strip_prefix('!')
            .or_else(|| tok.strip_prefix('-'))
            .filter(|t| is_pascal(t));
        if let Some(name) = negated {
            f.lacks.push(name.to_owned());
            continue;
        }
        if let Some((key, value)) = tok.split_once(':')
            && KEYS.contains(&key)
        {
            let value = value.to_owned();
            match key {
                "name" => f.name = Some(value),
                "parent" => f.parent = Some(value),
                "under" => f.under = Some(value),
                "prefab" => f.prefab = Some(value),
                "id" => f.ids.push(value),
                "has" => f.has.push(value),
                _ => f.lacks.push(value),
            }
            continue;
        }
        if let Some((op_text, op)) = OPS.iter().find(|(o, _)| tok.contains(o)) {
            let (lhs, rhs) = tok.split_once(op_text).unwrap_or((&tok, ""));
            if lhs == "depth" {
                let n: u32 = rhs
                    .parse()
                    .map_err(|_| invalid(q, format!("`{tok}`: depth must be a number")))?;
                f.max_depth = Some(match op {
                    CompareOp::Le => n,
                    CompareOp::Lt => n.saturating_sub(1),
                    _ => return Err(invalid(q, format!("`{tok}`: use `depth<=N` or `depth<N`"))),
                });
            } else {
                f.compare.push(Compare {
                    path: lhs.to_owned(),
                    op: *op,
                    value: literal(rhs),
                });
            }
            continue;
        }
        if is_pascal(&tok) {
            f.has.push(tok);
        } else if is_entity_address(&tok) {
            f.ids.push(tok);
        } else {
            return Err(invalid(
                q,
                format!(
                    "cannot read `{tok}`; terms are components (`Light`), `key:value` (name, parent, under, prefab, id), `depth<=N` or comparisons (`Light.intensity>500`)"
                ),
            ));
        }
    }
    check(&f, db, Some(q))?;
    Ok(f)
}

/// Checks component names and field paths of a filter.
pub fn check(f: &Filter, db: &SchemaDb, q: Option<&str>) -> Result<(), Diagnostic> {
    let shown = q.unwrap_or("filter");
    for name in f.has.iter().chain(&f.lacks) {
        if db.get(name).is_some_and(|t| t.is_component()) {
            continue;
        }
        let near = did_you_mean(name, db.components().map(|t| t.name.as_str()));
        let mut d = invalid(shown, format!("unknown component `{name}`"));
        if let (Some(near), Some(q)) = (near, q) {
            d.message.push_str(&format!("; did you mean `{near}`?"));
            d = d.with_fix(
                format!("Use `{near}`"),
                FixAction::Set {
                    record: None,
                    path: "q".into(),
                    value: q.replace(name.as_str(), near).into(),
                },
            );
        }
        return Err(d);
    }
    let entity = db
        .get("Entity")
        .ok_or_else(|| Diagnostic::error(codes::UNKNOWN_TYPE, "no `Entity` type"))?;
    for c in &f.compare {
        let p: FieldPath = c.path.parse().map_err(|e: String| invalid(shown, e))?;
        db.resolve_path(entity, &p)
            .map_err(|e| invalid(shown, e.message))?;
    }
    Ok(())
}

/// Merges two filters (all conditions must hold).
pub fn and(mut a: Filter, b: Filter) -> Filter {
    a.has.extend(b.has);
    a.lacks.extend(b.lacks);
    a.ids.extend(b.ids);
    a.compare.extend(b.compare);
    a.name = a.name.or(b.name);
    a.parent = a.parent.or(b.parent);
    a.under = a.under.or(b.under);
    a.prefab = a.prefab.or(b.prefab);
    a.max_depth = match (a.max_depth, b.max_depth) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    };
    a
}

/// Case-insensitive glob with `*` and `?`.
pub fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &t[1..]),
            (Some(a), Some(b)) => a == b && go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    go(&p, &t)
}

/// The effective (default-filled) value at a field path of an entity.
pub fn field(db: &SchemaDb, e: &ResolvedEntity, field_path: &str) -> Option<Value> {
    let p: FieldPath = field_path.parse().ok()?;
    let entity = db.get("Entity")?;
    let resolved = db.resolve_path(entity, &p).ok()?;
    if ara_doc::is_component_key(p.head()) && !e.record.contains_key(p.head()) {
        return None;
    }
    path::get_effective(&e.record, &resolved.steps)
}

fn compare(op: CompareOp, have: &Value, want: &Value) -> bool {
    let ord = match (have.as_f64(), want.as_f64()) {
        (Some(a), Some(b)) => a.partial_cmp(&b),
        _ => match (have.as_str(), want.as_str()) {
            (Some(a), Some(b)) => Some(a.cmp(b)),
            _ => None,
        },
    };
    match op {
        CompareOp::Eq => ara_json::json_eq(have, want),
        CompareOp::Ne => !ara_json::json_eq(have, want),
        CompareOp::Gt => ord.is_some_and(|o| o.is_gt()),
        CompareOp::Ge => ord.is_some_and(|o| o.is_ge()),
        CompareOp::Lt => ord.is_some_and(|o| o.is_lt()),
        CompareOp::Le => ord.is_some_and(|o| o.is_le()),
    }
}

/// True if the entity at `addr` (at `depth`) matches every condition of `f`.
pub fn matches(
    f: &Filter,
    db: &SchemaDb,
    tree: &Tree,
    prefab: Option<&str>,
    addr: &str,
    depth: u32,
) -> bool {
    let Some(e) = tree.get(addr) else {
        return false;
    };
    let name = e.record.get("name").and_then(Value::as_str).unwrap_or("");
    f.has.iter().all(|c| e.record.contains_key(c))
        && !f.lacks.iter().any(|c| e.record.contains_key(c))
        && f.ids.iter().all(|id| id == addr)
        && f.name.as_ref().is_none_or(|g| glob(g, name))
        && f.parent
            .as_ref()
            .is_none_or(|p| tree.parent(addr) == Some(p.as_str()))
        && f.under.as_ref().is_none_or(|u| tree.is_under(addr, u))
        && f.max_depth.is_none_or(|m| depth <= m)
        && f.prefab
            .as_ref()
            .is_none_or(|p| prefab.is_some_and(|have| have.contains(p.as_str())))
        && f.compare
            .iter()
            .all(|c| field(db, e, &c.path).is_some_and(|have| compare(c.op, &have, &c.value)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn db() -> SchemaDb {
        let mut db = SchemaDb::new();
        ara_doc::register(&mut db);
        ara_components::register(&mut db);
        db
    }

    #[test]
    fn compact_syntax_parses_into_the_json_form() {
        let f = parse(
            r#"Transform !Transform name:"Big Crate*" under:e_aa depth<2 Transform.position.y>=1 e_bb"#,
            &db(),
        )
        .expect("parses");
        assert_eq!(f.has, ["Transform"]);
        assert_eq!(f.lacks, ["Transform"]);
        assert_eq!(f.name.as_deref(), Some("Big Crate*"));
        assert_eq!(f.under.as_deref(), Some("e_aa"));
        assert_eq!(f.max_depth, Some(1));
        assert_eq!(f.ids, ["e_bb"]);
        assert_eq!(
            f.compare,
            [Compare {
                path: "Transform.position.y".into(),
                op: CompareOp::Ge,
                value: json!(1)
            }]
        );
    }

    #[test]
    fn bad_terms_are_located_with_fixes() {
        let d = parse("Transfrom", &db()).expect_err("typo");
        assert_eq!(d.code, codes::INVALID_QUERY);
        assert_eq!(d.fixes.len(), 1, "{d}");
        assert!(matches!(&d.fixes[0].action, FixAction::Set { value, .. } if value == "Transform"));
        assert!(parse("Transform.positon>1", &db()).is_err());
        assert!(parse("what?", &db()).is_err());
    }

    #[test]
    fn globs_are_case_insensitive() {
        assert!(glob("cr*e", "Crate"));
        assert!(glob("c?ate", "CRATE"));
        assert!(!glob("cr*x", "Crate"));
    }
}
