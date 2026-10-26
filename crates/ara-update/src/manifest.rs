use crate::{Error, MANIFEST, SIGNATURE};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Independent release streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Standalone launcher.
    Hub,
    /// Side-by-side engine distribution.
    Engine,
}
impl Kind {
    /// Tag prefix and release-directory name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Hub => "hub",
            Self::Engine => "engine",
        }
    }
}
/// Preview includes GitHub prereleases; stable excludes them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Production releases.
    #[default]
    Stable,
    /// Prereleases.
    Preview,
}
/// Asset metadata covered by the manifest signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asset {
    /// Portable basename.
    pub name: String,
    /// Exact download length.
    pub size: u64,
    /// Lowercase SHA-256 hexadecimal digest.
    pub sha256: String,
    /// HTTPS GitHub release asset URL.
    pub url: String,
}
/// Version-one signed wire contract; signature covers its exact canonical bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Protocol identifier: ara.release/1.
    pub format: String,
    /// Hub or engine.
    pub kind: Kind,
    /// Semantic version; also the install-directory basename.
    pub version: String,
    /// Source identity for same-version Hub previews; absent on legacy releases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<crate::BuildIdentity>,
    /// Signed channel, checked against GitHub's prerelease flag.
    pub channel: Channel,
    /// Minimum supported Hub version for engine installs.
    pub min_hub_version: String,
    /// Human-readable release notes.
    pub notes: String,
    /// Platform identifier (currently windows-x64).
    pub platform: String,
    /// Archives, installer and notices published with this release.
    pub assets: Vec<Asset>,
}
/// A manifest whose exact bytes passed an embedded trust root. Cannot be forged by deserialization.
#[derive(Debug, Clone)]
pub struct VerifiedRelease(pub(crate) Manifest, String, String);
impl VerifiedRelease {
    /// Signed metadata for display and compatibility checks.
    pub fn manifest(&self) -> &Manifest {
        &self.0
    }
    /// Exact signed bytes and detached signature for an independent updater verification.
    pub fn proof(&self) -> (&str, &str) {
        (&self.1, &self.2)
    }
    /// Select the portable distribution, never a symbols archive.
    pub fn archive(&self) -> Result<&Asset, Error> {
        self.0
            .assets
            .iter()
            .find(|a| a.name.ends_with("-windows-x64.zip") && !a.name.contains("symbols"))
            .ok_or_else(|| Error::new(MANIFEST, "release has no Windows x64 portable archive"))
    }
}
/// Parse a path-safe semantic version. Build metadata is disallowed in installation paths.
pub fn version(text: &str) -> Result<semver::Version, Error> {
    let parsed = semver::Version::parse(text)
        .map_err(|_| Error::new(MANIFEST, "invalid semantic version"))?;
    if !parsed.build.is_empty() || text.len() > 100 {
        return Err(Error::new(
            MANIFEST,
            "version must not contain build metadata",
        ));
    }
    Ok(parsed)
}
/// Embedded rotation list, augmented at compile time for release engineering.
pub fn trusted_keys() -> Vec<String> {
    include_str!("../public-keys.txt")
        .lines()
        .chain(
            option_env!("ARA_RELEASE_PUBLIC_KEYS")
                .unwrap_or("")
                .split(','),
        )
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(str::to_owned)
        .collect()
}
/// Canonical manifest bytes used by publisher and verifier.
pub fn canonical_manifest(manifest: &Manifest) -> Result<Vec<u8>, Error> {
    Ok(
        ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(manifest)?))
            .into_bytes(),
    )
}
/// Sign canonical bytes with an externally stored 32-byte seed (hexadecimal).
pub fn sign(bytes: &[u8], seed: &str) -> Result<(String, String), Error> {
    let raw: [u8; 32] = hex::decode(seed.trim())
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| {
            Error::new(
                SIGNATURE,
                "signing key must contain a 32-byte hexadecimal seed",
            )
        })?;
    let key = SigningKey::from_bytes(&raw);
    Ok((
        hex::encode(key.sign(bytes).to_bytes()),
        hex::encode(key.verifying_key().to_bytes()),
    ))
}
/// Authenticate before parsing or trusting any asset URL, length or hash.
pub fn verify(bytes: &[u8], signature: &[u8], keys: &[String]) -> Result<VerifiedRelease, Error> {
    let signature = std::str::from_utf8(signature)
        .ok()
        .and_then(|s| hex::decode(s.trim()).ok())
        .and_then(|b| Signature::from_slice(&b).ok())
        .ok_or_else(|| Error::new(SIGNATURE, "release signature is missing or malformed"))?;
    if !keys.iter().any(|text| {
        hex::decode(text)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .and_then(|b| VerifyingKey::from_bytes(&b).ok())
            .is_some_and(|key| key.verify_strict(bytes, &signature).is_ok())
    }) {
        return Err(Error::new(
            SIGNATURE,
            "release signature does not match any trusted public key",
        ));
    }
    let manifest: Manifest = serde_json::from_slice(bytes)?;
    version(&manifest.version)?;
    version(&manifest.min_hub_version)?;
    if let Some(build) = &manifest.build
        && (build.generation == 0
            || build.commit.len() != 40
            || !build.commit.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(Error::new(
            MANIFEST,
            "invalid release source build identity",
        ));
    }
    if manifest.format != "ara.release/1"
        || manifest.platform != "windows-x64"
        || manifest.assets.is_empty()
    {
        return Err(Error::new(
            MANIFEST,
            "unsupported release format/platform or empty asset list",
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    for a in &manifest.assets {
        if a.name.is_empty()
            || !a
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
            || a.name == "."
            || a.name == ".."
            || !names.insert(&a.name)
            || a.size == 0
            || a.sha256.len() != 64
            || hex::decode(&a.sha256).is_err()
        {
            return Err(Error::new(MANIFEST, "invalid or duplicate asset metadata"));
        }
    }
    Ok(VerifiedRelease(
        manifest,
        std::str::from_utf8(bytes)
            .map_err(|_| Error::new(MANIFEST, "manifest is not UTF-8"))?
            .to_owned(),
        hex::encode(signature.to_bytes()),
    ))
}
