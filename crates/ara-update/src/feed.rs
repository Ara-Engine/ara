use crate::{
    Channel, Error, MANIFEST, NETWORK, RATE_LIMIT, URL, VerifiedRelease, trusted_keys, verify,
    write_json,
};
use reqwest::{
    Url,
    blocking::{Client, Response},
    header,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

/// Persisted feed preference. Tokens are read only from the process environment.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct FeedConfig {
    /// Public GitHub owner/name; ARA_RELEASE_REPO takes precedence.
    pub repo: String,
    /// Hub update channel; preview also sees stable releases.
    pub channel: Channel,
    #[serde(skip)]
    pub(crate) api: Option<String>,
    #[serde(skip)]
    keys: Option<Vec<String>>,
    #[serde(skip)]
    #[cfg(any(test, feature = "test-support"))]
    mirror: bool,
}
impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            repo: crate::DEFAULT_REPOSITORY.into(),
            channel: Channel::Stable,
            api: None,
            keys: None,
            #[cfg(any(test, feature = "test-support"))]
            mirror: false,
        }
    }
}
impl FeedConfig {
    /// Load update.json, applying the repository environment override.
    pub fn load(directory: &Path) -> Result<Self, Error> {
        let mut config: Self = match fs::read(directory.join("update.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        if let Ok(repo) = std::env::var("ARA_RELEASE_REPO") {
            config.repo = repo;
        }
        // This override does not exist in shipping builds. Signed metadata keeps
        // its GitHub URLs; every request is still confined to one loopback origin.
        #[cfg(feature = "test-support")]
        if let Ok(api) = std::env::var("ARA_RELEASE_TEST_API") {
            config.api = Some(api);
            config.mirror = true;
        }
        config.validate()?;
        Ok(config)
    }
    /// Validate before interpolating an owner/name into an API or download URL.
    pub fn validate(&self) -> Result<(), Error> {
        if self.repo.split('/').count() != 2
            || self.repo.split('/').any(|p| {
                p.is_empty()
                    || p == "."
                    || p == ".."
                    || !p
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
        {
            return Err(Error::new(URL, "release repository must be owner/name"));
        }
        Ok(())
    }
    /// Explicit loopback-only test transport. Never available in production builds.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(api: String, keys: Vec<String>) -> Self {
        Self {
            api: Some(api),
            keys: Some(keys),
            ..Self::default()
        }
    }
}
/// Feed freshness, exposed even when the cached list is usable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FeedStatus {
    /// Online; all pages fetched or revalidated.
    Fresh,
    /// Offline fallback, signed cache reverified.
    Offline {
        /// Reason without request credentials.
        message: String,
    },
    /// Cached data while GitHub throttles requests.
    RateLimited {
        /// Unix seconds or Retry-After value supplied by GitHub.
        retry_after: Option<String>,
    },
}
/// Authenticated releases and their freshness.
#[derive(Debug, Clone)]
pub struct FeedSnapshot {
    /// Both release streams in descending semantic version order.
    pub releases: Vec<VerifiedRelease>,
    /// Online, offline cache or rate-limited cache.
    pub status: FeedStatus,
}
#[derive(Clone, Serialize, Deserialize)]
struct CachedRelease {
    bytes: String,
    signature: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    pages: Vec<PageCache>,
    releases: Vec<CachedRelease>,
}
#[derive(Serialize, Deserialize)]
struct PageCache {
    url: String,
    etag: Option<String>,
    body: String,
    next: Option<String>,
}
#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    prerelease: bool,
    draft: bool,
    assets: Vec<GithubAsset>,
}
#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    url: Option<String>,
}

/// Blocking I/O client: run on a worker, never the UI/service authority thread.
pub struct Feed {
    pub(crate) config: FeedConfig,
    client: Client,
    cache: PathBuf,
}
impl Feed {
    /// Build an HTTPS-only client with manual host-checked redirects.
    pub fn new(config: FeedConfig, directory: &Path) -> Result<Self, Error> {
        config.validate()?;
        fs::create_dir_all(directory)?;
        let identity = format!(
            "{}:{}",
            config.repo,
            config.api.as_deref().unwrap_or("https://api.github.com")
        );
        let cache = directory.join(format!(
            "{}.json",
            hex::encode(Sha256::digest(identity.as_bytes()))
        ));
        let builder = Client::builder()
            .user_agent("Ara-Hub/1")
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(2 * 3600));
        #[cfg(any(test, feature = "test-support"))]
        let builder = if config.api.is_some() {
            builder.no_proxy()
        } else {
            builder
        };
        let client = builder
            .build()
            .map_err(|_| Error::new(NETWORK, "cannot initialize HTTPS client"))?;
        Ok(Self {
            config,
            client,
            cache,
        })
    }
    pub(crate) fn url(&self, text: &str) -> Result<Url, Error> {
        let url = Url::parse(text).map_err(|_| Error::new(URL, "invalid release URL"))?;
        #[cfg(any(test, feature = "test-support"))]
        if let Some(base) = &self.config.api {
            let base = Url::parse(base).map_err(|_| Error::new(URL, "invalid test API"))?;
            if base.host_str() == Some("127.0.0.1")
                && url.origin() == base.origin()
                && url.scheme() == "http"
            {
                return Ok(url);
            }
            if self.config.mirror
                && base.host_str() == Some("127.0.0.1")
                && base.scheme() == "http"
                && url.scheme() == "https"
                && url.host_str() == Some("github.com")
                && url.port_or_known_default() == Some(443)
                && url.username().is_empty()
                && url.password().is_none()
                && url
                    .path()
                    .starts_with(&format!("/{}/releases/download/", self.config.repo))
            {
                return base
                    .join(url.path())
                    .map_err(|_| Error::new(URL, "invalid mock asset path"));
            }
            return Err(Error::new(
                URL,
                "test release traffic cannot leave its loopback fake server",
            ));
        }
        let host = url.host_str().unwrap_or_default();
        if url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(
                host,
                "api.github.com"
                    | "github.com"
                    | "objects.githubusercontent.com"
                    | "release-assets.githubusercontent.com"
                    | "github-releases.githubusercontent.com"
            )
        {
            return Err(Error::new(
                URL,
                "release URLs must use HTTPS on an approved GitHub host",
            ));
        }
        Ok(url)
    }
    pub(crate) fn request(
        &self,
        text: &str,
        etag: Option<&str>,
        range: Option<u64>,
    ) -> Result<Response, Error> {
        self.request_with_timeout(text, etag, range, Duration::from_secs(45))
    }
    fn request_with_timeout(
        &self,
        text: &str,
        etag: Option<&str>,
        range: Option<u64>,
        timeout: Duration,
    ) -> Result<Response, Error> {
        let mut url = self.url(text)?;
        for _ in 0..=crate::redirect::MAX_REDIRECTS {
            let mut request = self.client.get(url.clone()).timeout(timeout).header(
                header::ACCEPT,
                if url.path().contains("/releases/assets/") {
                    "application/octet-stream"
                } else {
                    "application/vnd.github+json"
                },
            );
            if let Some(value) =
                crate::redirect::authorization(&url, std::env::var("GITHUB_TOKEN").ok().as_deref())?
            {
                request = request.header(header::AUTHORIZATION, value);
            }
            if let Some(etag) = etag {
                request = request.header(header::IF_NONE_MATCH, etag);
            }
            if let Some(offset) = range {
                request = request.header(header::RANGE, format!("bytes={offset}-"));
            }
            let response = request
                .send()
                .map_err(|_| Error::new(NETWORK, "GitHub is unreachable; check your connection"))?;
            if response.status().is_redirection() && response.status().as_u16() != 304 {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(|| Error::new(URL, "redirect has no valid Location"))?;
                url = self.redirect(&url, location)?;
                continue;
            }
            return Ok(response);
        }
        Err(Error::new(URL, "too many release redirects"))
    }
    fn read_cache(&self) -> Result<Cache, Error> {
        match fs::read(&self.cache) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Cache::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub(crate) fn asset_url(&self, asset: &crate::Asset) -> Result<String, Error> {
        self.asset_location(asset, std::env::var_os("GITHUB_TOKEN").is_some())
    }
    pub(crate) fn asset_location(
        &self,
        asset: &crate::Asset,
        authenticated: bool,
    ) -> Result<String, Error> {
        let url = self.url(&asset.url)?;
        let prefix = format!("/{}/releases/download/", self.config.repo);
        if url.host_str() != Some("github.com") || !authenticated {
            return Ok(asset.url.clone());
        }
        let Some(path) = url.path().strip_prefix(&prefix) else {
            // A signed legacy URL can name the repository's previous owner.
            // Preserve it and follow anonymous HTTPS redirects; never rewrite signed metadata.
            return Ok(asset.url.clone());
        };
        let (tag, _) = path
            .split_once('/')
            .ok_or_else(|| Error::new(URL, "invalid release asset path"))?;
        let endpoint = format!(
            "https://api.github.com/repos/{}/releases/tags/{tag}",
            self.config.repo
        );
        let release: serde_json::Value = serde_json::from_slice(&bounded(
            self.request(&endpoint, None, None)?,
            8 * 1024 * 1024,
        )?)?;
        let id = release["assets"]
            .as_array()
            .and_then(|assets| {
                assets
                    .iter()
                    .find(|a| a["name"].as_str() == Some(&asset.name))
            })
            .and_then(|a| a["id"].as_u64())
            .ok_or_else(|| Error::new(MANIFEST, "private staging release has no matching asset"))?;
        // Reuse the configured repository alias. Its API redirect remains authenticated
        // even when GitHub's payload uses the new owner or numeric repository path.
        let api = format!(
            "https://api.github.com/repos/{}/releases/assets/{id}",
            self.config.repo
        );
        self.url(&api)?;
        Ok(api)
    }
    fn verified(&self, records: &[CachedRelease]) -> Result<Vec<VerifiedRelease>, Error> {
        let keys = self.config.keys.clone().unwrap_or_else(trusted_keys);
        let mut releases = records
            .iter()
            .map(|r| verify(r.bytes.as_bytes(), r.signature.as_bytes(), &keys))
            .collect::<Result<Vec<_>, _>>()?;
        for release in &releases {
            for asset in &release.manifest().assets {
                self.url(&asset.url)?;
            }
        }
        releases
            .sort_by_cached_key(|r| std::cmp::Reverse(crate::version(&r.manifest().version).ok()));
        let mut versions = std::collections::BTreeSet::new();
        releases
            .retain(|r| versions.insert((r.manifest().kind.name(), r.manifest().version.clone())));
        Ok(releases)
    }
    /// Paginate with ETags; on transport/rate-limit errors use only a reverified cache.
    pub fn check(&self) -> Result<FeedSnapshot, Error> {
        let old = self.read_cache()?;
        match self.fetch(&old) {
            Ok(cache) => {
                let releases = self.verified(&cache.releases)?;
                write_json(&self.cache, &cache)?;
                Ok(FeedSnapshot {
                    releases,
                    status: FeedStatus::Fresh,
                })
            }
            Err(error) if error.code == NETWORK || error.code == RATE_LIMIT => {
                if old.pages.is_empty() {
                    return Err(error);
                }
                let releases = self.verified(&old.releases)?;
                let status = if error.code == RATE_LIMIT {
                    FeedStatus::RateLimited {
                        retry_after: Some(error.message),
                    }
                } else {
                    FeedStatus::Offline {
                        message: error.message,
                    }
                };
                Ok(FeedSnapshot { releases, status })
            }
            Err(error) => Err(error),
        }
    }
    fn fetch(&self, old: &Cache) -> Result<Cache, Error> {
        let api = self
            .config
            .api
            .as_deref()
            .unwrap_or("https://api.github.com");
        let mut url = Some(format!(
            "{api}/repos/{}/releases?per_page=100",
            self.config.repo
        ));
        let mut cache = Cache::default();
        let mut unchanged = true;
        while let Some(page_url) = url.take() {
            if cache.pages.len() >= 200 || cache.pages.iter().any(|p| p.url == page_url) {
                return Err(Error::new(
                    MANIFEST,
                    "release pagination exceeded limit or contains a cycle",
                ));
            }
            let previous = old.pages.iter().find(|p| p.url == page_url);
            let response =
                self.request(&page_url, previous.and_then(|p| p.etag.as_deref()), None)?;
            let page = if response.status().as_u16() == 304 {
                let previous =
                    previous.ok_or_else(|| Error::new(MANIFEST, "304 without cached page"))?;
                PageCache {
                    url: previous.url.clone(),
                    etag: previous.etag.clone(),
                    body: previous.body.clone(),
                    next: previous.next.clone(),
                }
            } else {
                check_status(response.status(), response.headers())?;
                unchanged = false;
                let etag = response
                    .headers()
                    .get(header::ETAG)
                    .and_then(|h| h.to_str().ok())
                    .map(str::to_owned);
                let next = response
                    .headers()
                    .get(header::LINK)
                    .and_then(|h| h.to_str().ok())
                    .and_then(next_link);
                if let Some(next) = &next {
                    let parsed = self.url(next)?;
                    let current = self.url(response.url().as_str())?;
                    if parsed.origin() != current.origin() || parsed.path() != current.path() {
                        return Err(Error::new(
                            URL,
                            "pagination left the configured releases endpoint",
                        ));
                    }
                }
                PageCache {
                    url: page_url,
                    etag,
                    body: String::from_utf8(bounded(response, 8 * 1024 * 1024)?)
                        .map_err(|_| Error::new(MANIFEST, "feed is not UTF-8"))?,
                    next,
                }
            };
            url = page.next.clone();
            cache.pages.push(page);
        }
        if unchanged && cache.pages.len() == old.pages.len() {
            cache.releases = old.releases.clone();
            return Ok(cache);
        }
        for page in &cache.pages {
            for release in serde_json::from_str::<Vec<GithubRelease>>(&page.body)? {
                if release.draft
                    || !(release.tag_name.starts_with("hub-v")
                        || release.tag_name.starts_with("engine-v"))
                {
                    continue;
                }
                // Public asset URLs reserve the anonymous API quota for listing pages.
                let asset_url = |name: &str| {
                    release
                        .assets
                        .iter()
                        .find(|a| a.name == name)
                        .map(|a| {
                            if std::env::var_os("GITHUB_TOKEN").is_some() {
                                a.url.as_deref().unwrap_or(&a.browser_download_url)
                            } else {
                                &a.browser_download_url
                            }
                        })
                        .ok_or_else(|| {
                            Error::new(
                                crate::SIGNATURE,
                                format!("{} has no {name}", release.tag_name),
                            )
                        })
                };
                let bytes = bounded(
                    self.request(asset_url("release.json")?, None, None)?,
                    1024 * 1024,
                )?;
                let signature = bounded(
                    self.request(asset_url("release.json.sig")?, None, None)?,
                    1024,
                )?;
                let keys = self.config.keys.clone().unwrap_or_else(trusted_keys);
                let verified = verify(&bytes, &signature, &keys)?;
                let manifest = verified.manifest();
                if release.tag_name != format!("{}-v{}", manifest.kind.name(), manifest.version)
                    || release.prerelease != (manifest.channel == Channel::Preview)
                {
                    return Err(Error::new(
                        MANIFEST,
                        "signed kind/version/channel does not match GitHub release",
                    ));
                }
                cache.releases.push(CachedRelease {
                    bytes: String::from_utf8(bytes)
                        .map_err(|_| Error::new(MANIFEST, "manifest is not UTF-8"))?,
                    signature: String::from_utf8(signature)
                        .map_err(|_| Error::new(crate::SIGNATURE, "signature is not UTF-8"))?,
                });
            }
        }
        Ok(cache)
    }
}
fn next_link(header: &str) -> Option<String> {
    header
        .split(',')
        .find(|s| s.contains("rel=\"next\""))
        .and_then(|s| s.split_once('<')?.1.split_once('>'))
        .map(|(url, _)| url.to_owned())
}
pub(crate) fn check_status(
    status: reqwest::StatusCode,
    headers: &header::HeaderMap,
) -> Result<(), Error> {
    if matches!(status.as_u16(), 403 | 429) {
        return Err(Error::new(
            RATE_LIMIT,
            headers
                .get("x-ratelimit-reset")
                .or_else(|| headers.get(header::RETRY_AFTER))
                .and_then(|h| h.to_str().ok())
                .unwrap_or("GitHub rate limit; retry later"),
        ));
    }
    if !status.is_success() {
        return Err(Error::new(
            if status.is_server_error() {
                NETWORK
            } else {
                MANIFEST
            },
            format!("GitHub returned HTTP {}", status.as_u16()),
        ));
    }
    Ok(())
}
fn bounded(response: Response, max: u64) -> Result<Vec<u8>, Error> {
    check_status(response.status(), response.headers())?;
    let mut bytes = Vec::new();
    response.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(Error::new(
            crate::SIZE,
            "release metadata exceeds size limit",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod rehearsal_tests {
    use super::*;
    #[test]
    fn mirror_preserves_signed_github_urls_but_cannot_leave_loopback() {
        let root = crate::testing::scratch("release-mirror");
        let mut config = FeedConfig::for_test("http://127.0.0.1:12345".into(), vec![]);
        let asset =
            "https://github.com/Ara-Engine/ara/releases/download/hub-v0.0.1-alpha/release.json";
        let feed = Feed::new(config.clone(), &root).expect("feed");
        assert!(
            feed.url(asset).is_err(),
            "ordinary test transports do not rewrite URLs"
        );
        config.mirror = true;
        let feed = Feed::new(config, &root).expect("mirror");
        assert_eq!(
            feed.url(asset).expect("mapped URL").as_str(),
            "http://127.0.0.1:12345/Ara-Engine/ara/releases/download/hub-v0.0.1-alpha/release.json"
        );
        for url in [
            "https://example.com/secret",
            "https://github.com/Other/repo/releases/download/x/file",
            "https://user:secret@github.com/Ara-Engine/ara/releases/download/x/file",
            "http://127.0.0.1:12346/file",
        ] {
            assert!(feed.url(url).is_err(), "escaped mirror: {url}");
        }
    }
}
