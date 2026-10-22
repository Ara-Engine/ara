//! Lexical rules for names, IDs and `@path#id` references.
//!
//! - type names: PascalCase (`Transform`); field and variant names: camelCase;
//! - IDs: `<prefix>_<body>`, prefix 1–3 lowercase letters, body 1–24 of `[0-9a-z]`;
//! - entity addresses: record IDs joined by `/` (`e_4f9a/m4tz`), the path from a
//!   document record down into nested prefab instances;
//! - references: `@path#id`, `@path` (ID filled on read) or `@#id` (path filled).

/// PascalCase type name: `[A-Z][A-Za-z0-9]*`.
pub fn is_type_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.all(|c| c.is_ascii_alphanumeric())
}

/// camelCase field or variant name: `[a-z][A-Za-z0-9]*`.
pub fn is_field_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_alphanumeric())
}

/// Document kind name: `[a-z][a-z0-9]*`.
pub fn is_kind_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// ID prefix: 1–3 lowercase letters.
pub fn is_id_prefix(s: &str) -> bool {
    (1..=3).contains(&s.len()) && s.chars().all(|c| c.is_ascii_lowercase())
}

/// Splits a well-formed ID into `(prefix, body)`.
pub fn split_id(s: &str) -> Option<(&str, &str)> {
    let (prefix, body) = s.split_once('_')?;
    let body_ok = (1..=24).contains(&body.len())
        && body
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    (is_id_prefix(prefix) && body_ok).then_some((prefix, body))
}

/// Record IDs joined by `/`.
pub fn is_entity_address(s: &str) -> bool {
    !s.is_empty() && s.split('/').all(|part| split_id(part).is_some())
}

/// The two halves of an `@path#id` reference; at least one is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefParts<'a> {
    /// Project-relative path, `/`-separated.
    pub path: Option<&'a str>,
    /// Asset ID.
    pub id: Option<&'a str>,
}

/// Parses `@path#id`, `@path` or `@#id`.
pub fn parse_ref(s: &str) -> Result<RefParts<'_>, String> {
    let rest = s
        .strip_prefix('@')
        .ok_or_else(|| format!("reference `{s}` must start with `@`"))?;
    let (path, id) = match rest.split_once('#') {
        Some((p, i)) => (p, Some(i)),
        None => (rest, None),
    };
    let path = (!path.is_empty()).then_some(path);
    if let Some(p) = path {
        let bad = p.starts_with('/')
            || p.contains('\\')
            || p.split('/').any(|seg| seg.is_empty() || seg == "..");
        if bad {
            return Err(format!(
                "reference path `{p}` must be project-relative with `/` separators"
            ));
        }
    }
    if let Some(i) = id
        && split_id(i).is_none()
    {
        return Err(format!("reference ID `{i}` is not a valid asset ID"));
    }
    if path.is_none() && id.is_none() {
        return Err("empty reference `@`".to_owned());
    }
    Ok(RefParts { path, id })
}

/// Formats a full reference.
pub fn format_ref(path: &str, id: &str) -> String {
    format!("@{path}#{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_addresses() {
        assert_eq!(split_id("e_8b21"), Some(("e", "8b21")));
        assert_eq!(split_id("pf_91fd02ab"), Some(("pf", "91fd02ab")));
        assert!(
            split_id("E_1").is_none() && split_id("e_").is_none() && split_id("abcd_1").is_none()
        );
        assert!(is_entity_address("e_4f9a/m_4tz"));
        assert!(!is_entity_address("e_4f9a/"));
    }

    #[test]
    fn references() {
        let full = parse_ref("@prefabs/rifle.prefab#pf_91fd").expect("valid");
        assert_eq!(
            (full.path, full.id),
            (Some("prefabs/rifle.prefab"), Some("pf_91fd"))
        );
        assert_eq!(parse_ref("@prefabs/rifle.prefab").expect("valid").id, None);
        assert_eq!(parse_ref("@#pf_91fd").expect("valid").path, None);
        for bad in ["prefabs/a", "@", "@#", "@/abs#pf_1", "@a/../b", "@a#Bad"] {
            assert!(parse_ref(bad).is_err(), "{bad}");
        }
    }
}
