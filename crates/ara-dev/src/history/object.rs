//! Commit/tag headers retain their raw bytes except mapped object references.
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

pub(super) fn parts(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let i = bytes
        .windows(2)
        .position(|v| v == b"\n\n")
        .context("object has no message separator")?;
    Ok((&bytes[..i + 1], &bytes[i + 2..]))
}
pub(super) fn links(bytes: &[u8], key: &[u8]) -> Result<Vec<String>> {
    parts(bytes)?
        .0
        .split_inclusive(|v| *v == b'\n')
        .filter_map(|line| line.strip_prefix(key))
        .map(|value| {
            let value = value.strip_suffix(b"\n").context("object link newline")?;
            ensure!(
                matches!(value.len(), 40 | 64) && value.iter().all(u8::is_ascii_hexdigit),
                "invalid object id"
            );
            Ok(std::str::from_utf8(value)?.to_owned())
        })
        .collect()
}
pub(super) fn rewrite(bytes: &[u8], key: &[u8], map: &BTreeMap<String, String>) -> Result<Vec<u8>> {
    let (header, message) = parts(bytes)?;
    let mut rewritten = Vec::new();
    for line in header.split_inclusive(|v| *v == b'\n') {
        if let Some(old) = line.strip_prefix(key) {
            let old = std::str::from_utf8(old.strip_suffix(b"\n").context("link newline")?)?;
            rewritten.extend_from_slice(key);
            rewritten.extend_from_slice(
                map.get(old)
                    .with_context(|| format!("unmapped parent/target {old}"))?
                    .as_bytes(),
            );
            rewritten.push(b'\n');
        } else {
            rewritten.extend_from_slice(line);
        }
    }
    rewritten.push(b'\n');
    rewritten.extend(super::message::clean(message)?);
    Ok(rewritten)
}
pub(super) fn signed(kind: &str, bytes: &[u8]) -> Result<bool> {
    let (header, message) = parts(bytes)?;
    Ok(header
        .split(|v| *v == b'\n')
        .any(|v| v.starts_with(b"gpgsig ") || v.starts_with(b"gpgsig-sha256 "))
        || (kind == "tag"
            && [
                b"-----BEGIN PGP SIGNATURE-----".as_slice(),
                b"-----BEGIN SSH SIGNATURE-----",
                b"-----BEGIN SIGNED MESSAGE-----",
            ]
            .iter()
            .any(|mark| message.windows(mark.len()).any(|v| v == *mark))))
}
pub(super) fn verify(
    old: &[u8],
    new: &[u8],
    key: &[u8],
    map: &BTreeMap<String, String>,
) -> Result<()> {
    let (before_header, before_message) = parts(old)?;
    let (after_header, after_message) = parts(new)?;
    let retained = |header: &[u8]| -> Vec<u8> {
        header
            .split_inclusive(|v| *v == b'\n')
            .filter(|line| !line.starts_with(key))
            .flatten()
            .copied()
            .collect()
    };
    ensure!(
        retained(before_header) == retained(after_header),
        "tree/author/committer/date/other header bytes changed"
    );
    let expected: Result<Vec<_>> = links(old, key)?
        .iter()
        .map(|id| {
            map.get(id)
                .cloned()
                .context("verification map missing parent/target")
        })
        .collect();
    ensure!(
        links(new, key)? == expected?,
        "ordered parent/target topology changed"
    );
    ensure!(
        after_message == super::message::clean(before_message)?,
        "unexpected message byte changes"
    );
    ensure!(
        super::message::clean(after_message)? == after_message,
        "remaining attribution"
    );
    Ok(())
}

/// Message-match recovery also checks tree, author/date and other retained headers.
pub(super) fn identity(bytes: &[u8]) -> Result<Vec<u8>> {
    let (headers, message) = parts(bytes)?;
    let mut identity: Vec<u8> = headers
        .split_inclusive(|v| *v == b'\n')
        .filter(|line| !line.starts_with(b"parent "))
        .flatten()
        .copied()
        .collect();
    identity.push(b'\n');
    identity.extend(super::message::clean(message)?);
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_headers_encoding_signatures_and_parent_order_survive() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let map = BTreeMap::from([(a.clone(), b.clone()), (b.clone(), a.clone())]);
        let old = format!("tree {a}\nparent {a}\nparent {b}\nauthor A <a@b> 12 +0300\ncommitter C <c@d> 34 -0400\nencoding ISO-8859-1\ngpgsig fake\n continuation\n\nBody\n\nCo-Authored-By: Claude\n").into_bytes();
        let new = rewrite(&old, b"parent ", &map).unwrap();
        verify(&old, &new, b"parent ", &map).unwrap();
        assert_eq!(links(&new, b"parent ").unwrap(), vec![b, a]);
        assert!(signed("commit", &new).unwrap());
        let altered = String::from_utf8(new)
            .unwrap()
            .replace("12 +0300", "13 +0300");
        assert!(verify(&old, altered.as_bytes(), b"parent ", &map).is_err());
    }
}
