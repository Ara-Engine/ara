//! Ordered source identity for same-version Hub previews.
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::Path;

/// Read an installed standalone Hub marker, including legacy version-only markers.
pub fn read(path: &Path) -> Result<(String, Option<BuildIdentity>), crate::Error> {
    let bytes = std::fs::read(path)?;
    let value: serde_json::Value =
        serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))?;
    if value["format"] != "ara.hub/1" {
        return Err(crate::Error::new(
            crate::MANIFEST,
            "not an Ara Hub installation",
        ));
    }
    let version = value["version"]
        .as_str()
        .ok_or_else(|| crate::Error::new(crate::MANIFEST, "Hub version is missing"))?;
    crate::version(version)?;
    Ok((
        version.into(),
        serde_json::from_value(value["build"].clone())?,
    ))
}

/// A source commit and its monotonically increasing mainline ancestry count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildIdentity {
    /// Full Git commit ID, embedded in the executable and covered by the release signature.
    pub commit: String,
    /// `git rev-list --count HEAD`; increases along the publishing branch.
    pub generation: u64,
}

/// Compare releases by semver, then source generation. Legacy builds have no identity.
pub fn newer(
    version: &str,
    build: Option<&BuildIdentity>,
    current: &str,
    installed: Option<&BuildIdentity>,
) -> bool {
    match crate::version(version)
        .ok()
        .cmp(&crate::version(current).ok())
    {
        Ordering::Greater => true,
        Ordering::Equal => match (build, installed) {
            (Some(_), None) => true,
            (Some(new), Some(old)) => new.commit != old.commit && new.generation > old.generation,
            _ => false,
        },
        Ordering::Less => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_order_uses_source_identity_and_never_downgrades_a_newer_hub() {
        let old = BuildIdentity {
            commit: "a".repeat(40),
            generation: 10,
        };
        let new = BuildIdentity {
            commit: "b".repeat(40),
            generation: 11,
        };
        assert!(newer("0.0.1-alpha", Some(&new), "0.0.1-alpha", Some(&old)));
        assert!(!newer("0.0.1-alpha", Some(&old), "0.0.1-alpha", Some(&new)));
        assert!(!newer("0.0.1-alpha", Some(&new), "0.0.1-alpha", Some(&new)));
        assert!(newer("0.0.1-alpha", Some(&new), "0.0.1-alpha", None));
        assert!(!newer("0.0.1-alpha", None, "0.0.1-alpha", Some(&new)));
    }
}
