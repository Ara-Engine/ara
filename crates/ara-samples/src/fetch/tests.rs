#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("ARA_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-samples/tests")
            })
            .join(format!(
                "sample-mirrors-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("project")).unwrap();
        std::fs::write(
            root.join("project/ara.project.json"),
            r#"{"layoutVersion":4}"#,
        )
        .unwrap();
        Self(root)
    }

    fn url(&self, name: &str) -> String {
        let path = self.0.join(name).to_string_lossy().replace('\\', "/");
        // Preserve the drive separator; encode characters meaningful inside a URL.
        let path = path
            .replace('%', "%25")
            .replace(' ', "%20")
            .replace('#', "%23");
        if cfg!(windows) {
            format!("file:///{path}")
        } else {
            format!("file://{path}")
        }
    }

    fn asset(&self, primary: &str, mirrors: &[&str], expected: &[u8]) -> Asset {
        serde_json::from_value(serde_json::json!({
            "id": "pack", "title": "Pack", "kind": "audio", "source": "https://example.invalid",
            "license": "CC0-1.0", "authors": ["Original author"], "projects": ["project"],
            "files": [{"path": "clip.wav", "url": self.url(primary),
                "mirrors": mirrors.iter().map(|m| self.url(m)).collect::<Vec<_>>(),
                "size": expected.len(), "blake3": blake3::hash(expected).to_hex().to_string()}]
        }))
        .unwrap()
    }

    fn cache(&self) -> Cache {
        Cache::new(&self.0)
    }

    fn target(&self, asset: &Asset) -> PathBuf {
        asset.dir(&self.0.join("project")).join("clip.wav")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn dead_primary_falls_through_to_a_verified_mirror() {
    let fixture = Fixture::new();
    let bytes = b"pinned sound bytes";
    std::fs::write(fixture.0.join("mirror.wav"), bytes).unwrap();
    let asset = fixture.asset("missing-primary.wav", &["mirror.wav"], bytes);
    materialize(&fixture.cache(), &fixture.0.join("project"), &asset).unwrap();
    assert_eq!(std::fs::read(fixture.target(&asset)).unwrap(), bytes);
    assert_eq!(
        std::fs::read(fixture.cache().blob(&asset.files[0].blake3)).unwrap(),
        bytes
    );
    let lock = Lock {
        version: 1,
        assets: vec![asset],
    };
    assert!(verify(&fixture.0, &lock, &[]).unwrap().is_empty());
}

#[test]
fn wrong_hash_mirror_fails_without_publishing_a_blob() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("mirror.wav"), b"untrusted bytes").unwrap();
    let asset = fixture.asset(
        "missing-primary.wav",
        &["mirror.wav"],
        b"pinned sound bytes",
    );
    let error = materialize(&fixture.cache(), &fixture.0.join("project"), &asset).unwrap_err();
    assert!(format!("{error:#}").contains("hash mismatch"), "{error:#}");
    assert!(!fixture.target(&asset).exists());
    assert_eq!(
        std::fs::read_dir(fixture.0.join(".cache/blobs"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        std::fs::read_dir(fixture.0.join(".cache/partial"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn all_dead_reports_each_url_and_its_curl_failure_in_attempt_order() {
    let fixture = Fixture::new();
    let asset = fixture.asset(
        "primary.wav",
        &["mirror-one.wav", "mirror-two.wav"],
        b"pinned",
    );
    let error = materialize(&fixture.cache(), &fixture.0.join("project"), &asset).unwrap_err();
    let error = format!("{error:#}");
    let mut previous = 0;
    for url in std::iter::once(&asset.files[0].url).chain(&asset.files[0].mirrors) {
        let position = error
            .find(url)
            .unwrap_or_else(|| panic!("missing {url}: {error}"));
        assert!(position >= previous, "{error}");
        previous = position;
        let failure = error[position..].lines().next().unwrap();
        assert!(
            failure.contains("curl") && failure.contains("37"),
            "{failure}"
        );
    }
}

#[test]
fn invalid_mirror_is_skipped_and_size_is_checked_before_caching() {
    let fixture = Fixture::new();
    let bytes = b"pinned sound bytes";
    std::fs::write(fixture.0.join("bad.wav"), b"tampered bytes").unwrap();
    std::fs::write(fixture.0.join("good.wav"), bytes).unwrap();
    let mut asset = fixture.asset("missing.wav", &["bad.wav", "good.wav"], bytes);
    materialize(&fixture.cache(), &fixture.0.join("project"), &asset).unwrap();
    assert_eq!(std::fs::read(fixture.target(&asset)).unwrap(), bytes);
    // The same bytes with an incorrect size pin must not pass verify or enter a fresh cache.
    asset.files[0].size += 1;
    let lock = Lock {
        version: 1,
        assets: vec![asset.clone()],
    };
    assert_eq!(verify(&fixture.0, &lock, &[]).unwrap().len(), 1);
    std::fs::remove_file(fixture.target(&asset)).unwrap();
    std::fs::remove_file(fixture.cache().blob(&asset.files[0].blake3)).unwrap();
    let error = materialize(&fixture.cache(), &fixture.0.join("project"), &asset).unwrap_err();
    assert!(format!("{error:#}").contains("size mismatch"), "{error:#}");
    assert_eq!(
        std::fs::read_dir(fixture.0.join(".cache/blobs"))
            .unwrap()
            .count(),
        0
    );
}
