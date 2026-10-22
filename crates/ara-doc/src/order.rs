//! Fractional-index sibling order keys (`a0`, `a0V`, `Zz`).
//!
//! Siblings sort by `order` (byte-wise), then by record ID. Inserting between two
//! siblings mints a key between theirs, so a reorder or reparent rewrites one line.
//! Keys use base-62 digits with a variable-length integer head, so appending at
//! either end grows keys logarithmically. This follows the published
//! "fractional-indexing" algorithm (Figma; rocicorp, CC0), reimplemented here.

use std::fmt;

use serde::{Deserialize, Serialize};

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: usize = 62;
/// The key `key_between(None, None)` returns.
const ZERO: &str = "a0";
/// The one integer part with nothing below it; not itself a valid key.
const SMALLEST: &str = "A00000000000000000000000000";

/// A validated order key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OrderKey(String);

impl OrderKey {
    /// Parses and validates a key.
    pub fn parse(s: &str) -> Result<Self, String> {
        validate(s).map(|()| OrderKey(s.to_owned()))
    }

    /// The key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for OrderKey {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        OrderKey::parse(&s)
    }
}

impl From<OrderKey> for String {
    fn from(k: OrderKey) -> String {
        k.0
    }
}

impl fmt::Display for OrderKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn digit(c: u8) -> Result<usize, String> {
    DIGITS
        .iter()
        .position(|&d| d == c)
        .ok_or_else(|| format!("`{}` is not an order-key digit", char::from(c)))
}

fn int_len(head: u8) -> Result<usize, String> {
    match head {
        b'a'..=b'z' => Ok(usize::from(head - b'a') + 2),
        b'A'..=b'Z' => Ok(usize::from(b'Z' - head) + 2),
        _ => Err(format!("`{}` is not an order-key head", char::from(head))),
    }
}

fn int_part(key: &str) -> Result<&str, String> {
    let head = *key.as_bytes().first().ok_or("empty order key")?;
    let len = int_len(head)?;
    key.get(..len)
        .ok_or_else(|| format!("order key `{key}` is too short"))
}

fn validate(key: &str) -> Result<(), String> {
    if key == SMALLEST {
        return Err("order key has nothing below it".to_owned());
    }
    for c in key.bytes() {
        digit(c)?;
    }
    let int = int_part(key)?;
    if key[int.len()..].ends_with('0') {
        return Err(format!("order key `{key}` has a trailing zero"));
    }
    Ok(())
}

/// A fractional part strictly between `a` (may be empty) and `b` (`None` = above all).
fn midpoint(a: &str, b: Option<&str>) -> Result<String, String> {
    if let Some(b) = b {
        if a >= b {
            return Err(format!("`{a}` is not below `{b}`"));
        }
        let (ab, bb) = (a.as_bytes(), b.as_bytes());
        let mut n = 0;
        while n < bb.len() && ab.get(n).copied().unwrap_or(b'0') == bb[n] {
            n += 1;
        }
        if n > 0 {
            let rest = midpoint(a.get(n..).unwrap_or(""), Some(&b[n..]))?;
            return Ok(format!("{}{rest}", &b[..n]));
        }
    }
    let da = match a.as_bytes().first() {
        Some(&c) => digit(c)?,
        None => 0,
    };
    let db = match b {
        Some(b) => digit(b.as_bytes()[0])?,
        None => BASE,
    };
    if db.saturating_sub(da) > 1 {
        return Ok(char::from(DIGITS[(da + db).div_ceil(2)]).to_string());
    }
    match b {
        Some(b) if b.len() > 1 => Ok(b[..1].to_owned()),
        _ => Ok(format!(
            "{}{}",
            char::from(DIGITS[da]),
            midpoint(a.get(1..).unwrap_or(""), None)?
        )),
    }
}

fn increment(int: &str) -> Result<Option<String>, String> {
    let head = int.as_bytes()[0];
    let mut digs = int.as_bytes()[1..].to_vec();
    let mut carry = true;
    for d in digs.iter_mut().rev() {
        let next = digit(*d)? + 1;
        if next == BASE {
            *d = b'0';
        } else {
            *d = DIGITS[next];
            carry = false;
            break;
        }
    }
    if carry {
        if head == b'Z' {
            return Ok(Some(ZERO.to_owned()));
        }
        if head == b'z' {
            return Ok(None);
        }
        let h = head + 1;
        if h > b'a' {
            digs.push(b'0');
        } else {
            digs.pop();
        }
        return Ok(Some(join(h, &digs)));
    }
    Ok(Some(join(head, &digs)))
}

fn decrement(int: &str) -> Result<Option<String>, String> {
    let head = int.as_bytes()[0];
    let mut digs = int.as_bytes()[1..].to_vec();
    let mut borrow = true;
    for d in digs.iter_mut().rev() {
        let v = digit(*d)?;
        if v == 0 {
            *d = DIGITS[BASE - 1];
        } else {
            *d = DIGITS[v - 1];
            borrow = false;
            break;
        }
    }
    if borrow {
        if head == b'a' {
            return Ok(Some(join(b'Z', &[DIGITS[BASE - 1]])));
        }
        if head == b'A' {
            return Ok(None);
        }
        let h = head - 1;
        if h < b'Z' {
            digs.push(DIGITS[BASE - 1]);
        } else {
            digs.pop();
        }
        return Ok(Some(join(h, &digs)));
    }
    Ok(Some(join(head, &digs)))
}

fn join(head: u8, digs: &[u8]) -> String {
    std::iter::once(head)
        .chain(digs.iter().copied())
        .map(char::from)
        .collect()
}

/// A key strictly between `a` and `b`. `None` means "before everything" / "after
/// everything". Fails if `a >= b`.
pub fn key_between(a: Option<&OrderKey>, b: Option<&OrderKey>) -> Result<OrderKey, String> {
    let key = match (a.map(OrderKey::as_str), b.map(OrderKey::as_str)) {
        (None, None) => ZERO.to_owned(),
        (Some(a), Some(b)) if a >= b => return Err(format!("order key `{a}` is not below `{b}`")),
        (None, Some(b)) => {
            let ib = int_part(b)?;
            let fb = &b[ib.len()..];
            if ib == SMALLEST {
                format!("{ib}{}", midpoint("", Some(fb))?)
            } else if !fb.is_empty() {
                ib.to_owned()
            } else {
                decrement(ib)?.ok_or("order keys exhausted below")?
            }
        }
        (Some(a), None) => {
            let ia = int_part(a)?;
            match increment(ia)? {
                Some(i) => i,
                None => format!("{ia}{}", midpoint(&a[ia.len()..], None)?),
            }
        }
        (Some(a), Some(b)) => {
            let (ia, ib) = (int_part(a)?, int_part(b)?);
            let (fa, fb) = (&a[ia.len()..], &b[ib.len()..]);
            if ia == ib {
                format!("{ia}{}", midpoint(fa, Some(fb))?)
            } else {
                let i = increment(ia)?.ok_or("order keys exhausted above")?;
                if i.as_str() < b {
                    i
                } else {
                    format!("{ia}{}", midpoint(fa, None)?)
                }
            }
        }
    };
    OrderKey::parse(&key)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn k(s: &str) -> OrderKey {
        OrderKey::parse(s).expect("valid key")
    }

    #[test]
    fn known_keys() {
        assert_eq!(key_between(None, None).expect("key").as_str(), "a0");
        assert_eq!(
            key_between(Some(&k("a0")), None).expect("key").as_str(),
            "a1"
        );
        assert_eq!(
            key_between(None, Some(&k("a0"))).expect("key").as_str(),
            "Zz"
        );
        assert_eq!(
            key_between(Some(&k("a0")), Some(&k("a1")))
                .expect("key")
                .as_str(),
            "a0V"
        );
        assert_eq!(
            key_between(Some(&k("az")), None).expect("key").as_str(),
            "b00"
        );
        assert!(key_between(Some(&k("a1")), Some(&k("a0"))).is_err());
        assert!(
            OrderKey::parse("a10").is_err()
                && OrderKey::parse("a").is_err()
                && OrderKey::parse("a!").is_err()
        );
    }

    proptest! {
        /// Any sequence of insertions keeps keys valid and strictly ordered.
        #[test]
        fn inserts_stay_ordered(positions in prop::collection::vec(any::<prop::sample::Index>(), 1..200)) {
            let mut keys: Vec<OrderKey> = Vec::new();
            for pos in positions {
                let i = pos.index(keys.len() + 1);
                let key = key_between(i.checked_sub(1).map(|j| &keys[j]), keys.get(i)).expect("room between keys");
                keys.insert(i, key);
            }
            for pair in keys.windows(2) {
                prop_assert!(pair[0] < pair[1], "{} !< {}", pair[0], pair[1]);
            }
        }

        /// Appending stays short.
        #[test]
        fn appends_grow_slowly(n in 1usize..2000) {
            let mut last: Option<OrderKey> = None;
            for _ in 0..n {
                last = Some(key_between(last.as_ref(), None).expect("key"));
            }
            prop_assert!(last.expect("appended").as_str().len() <= 4);
        }
    }
}
