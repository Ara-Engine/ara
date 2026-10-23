use crate::{Locale, Result};

/// Resolve `loc:key` and apply deterministic accents plus expansion for a pseudo-locale.
/// Literal text also expands; model values, references and action names never pass through here.
pub fn localize(value: &str, locale: &Locale) -> Result<String> {
    let value = if let Some(key) = value.strip_prefix("loc:") {
        locale
            .messages
            .get(key)
            .ok_or_else(|| format!("locale {} has no message {key}", locale.id))?
    } else {
        value
    };
    if !locale.pseudo || value.is_empty() {
        return Ok(value.into());
    }
    let mut out = String::from("[");
    for c in value.chars() {
        out.push(match c {
            'a' => 'á',
            'e' => 'ë',
            'i' => 'ï',
            'o' => 'ô',
            'u' => 'ü',
            'A' => 'Á',
            'E' => 'Ë',
            'I' => 'Ï',
            'O' => 'Ô',
            'U' => 'Ü',
            'c' => 'ç',
            'n' => 'ñ',
            's' => 'š',
            'y' => 'ý',
            _ => c,
        });
    }
    out.push_str(&"~".repeat((value.chars().count() * 35).div_ceil(100)));
    out.push(']');
    Ok(out)
}
