#![allow(clippy::unwrap_used)]
use ara_asset::*;
use ara_importers::TextureImporter;
use image::ImageEncoder;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct GateCache {
    cache: FileCache,
    final_key: Digest,
    fast_key: Digest,
    paused: AtomicBool,
    pause_fast: AtomicBool,
    entered: AtomicBool,
    fail: bool,
    fast_loads: AtomicUsize,
}
impl CacheBackend for GateCache {
    fn load(&self, key: Digest) -> Result<Option<CachedImport>> {
        if key == self.fast_key {
            self.fast_loads.fetch_add(1, Ordering::Relaxed);
        }
        self.cache.load(key)
    }
    fn store(&self, key: Digest, output: &ImportOutput) -> Result<()> {
        if key == self.fast_key {
            let start = Instant::now();
            while self.pause_fast.load(Ordering::Acquire) {
                assert!(start.elapsed() < Duration::from_secs(15));
                thread::sleep(Duration::from_millis(2));
            }
        }
        if key == self.final_key {
            self.entered.store(true, Ordering::Release);
            let start = Instant::now();
            while self.paused.load(Ordering::Acquire) {
                assert!(
                    start.elapsed() < Duration::from_secs(15),
                    "test must release HQ store"
                );
                thread::sleep(Duration::from_millis(2));
            }
            if self.fail {
                return Err(Error::Invalid("injected HQ failure".into()));
            }
        }
        self.cache.store(key, output)
    }
}
fn input() -> ImportInput {
    let mut source = Vec::new();
    image::codecs::png::PngEncoder::new(&mut source)
        .write_image(
            &[45, 87, 91, 255].repeat(64),
            8,
            8,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    ImportInput {
        preset: CookPreset::HighQuality,
        thread_budget: 2,
        source_path: "color.png".into(),
        source,
        importer: TextureImporter.id().into(),
        version: TextureImporter.version().into(),
        settings: serde_json::json!({}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    }
}
fn wait(reload: &mut HotReload, failed: bool) -> Vec<ReloadEvent> {
    let start = Instant::now();
    loop {
        let events = reload.poll();
        if events.iter().any(|e| {
            if failed {
                matches!(e, ReloadEvent::Failed { .. })
            } else {
                matches!(e, ReloadEvent::Ready { .. })
            }
        }) {
            return events;
        }
        assert!(start.elapsed() < Duration::from_secs(15));
        thread::sleep(Duration::from_millis(2));
    }
}
fn current(reload: &HotReload) -> Digest {
    reload
        .slot("as_color")
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .key
}
fn setup(fail: bool) -> (tempfile::TempDir, ImportInput, Arc<GateCache>, HotReload) {
    let dir = tempfile::tempdir().unwrap();
    let input = input();
    let mut fast = input.clone();
    fast.preset = CookPreset::Fast;
    let cache = Arc::new(GateCache {
        cache: FileCache::new(dir.path()).unwrap(),
        final_key: input.key().unwrap(),
        fast_key: fast.key().unwrap(),
        paused: AtomicBool::new(true),
        pause_fast: AtomicBool::new(false),
        entered: AtomicBool::new(false),
        fail,
        fast_loads: AtomicUsize::new(0),
    });
    let reload = HotReload::new(
        // These tests gate CPU action persistence/boundaries, independently of
        // available Vulkan hardware. GPU protocol/byte equality has its own probes.
        ProcessWorker::new(env!("CARGO_BIN_EXE_ara-import-worker")).with_texture_encoder(false),
        cache.clone(),
    );
    (dir, input, cache, reload)
}
#[test]
fn preview_publication_does_not_wait_for_disk_persistence() {
    let (_dir, input, cache, mut reload) = setup(false);
    cache.pause_fast.store(true, Ordering::Release);
    reload.request("as_color", input);
    assert!(wait(&mut reload, false).iter().any(|e| matches!(
        e,
        ReloadEvent::Ready {
            final_pass: false,
            ..
        }
    )));
    assert!(cache.load(cache.fast_key).unwrap().is_none());
    assert!(reload.is_importing());
    reload.apply_boundary(true, false, &BTreeSet::new());
    assert_eq!(current(&reload), cache.fast_key);
    cache.pause_fast.store(false, Ordering::Release);
    cache.paused.store(false, Ordering::Release);
    assert!(wait(&mut reload, false).iter().any(|e| matches!(
        e,
        ReloadEvent::Ready {
            final_pass: true,
            ..
        }
    )));
}
#[test]
fn fast_then_hq_swaps_only_at_permitted_boundaries_and_warm_hq_skips_preview() {
    let (_dir, input, cache, mut reload) = setup(false);
    reload.request("as_color", input.clone());
    wait(&mut reload, false);
    assert!(reload.is_importing());
    assert!(reload.slot("as_color").is_none());
    assert!(
        reload
            .apply_boundary(true, true, &BTreeSet::new())
            .is_empty()
    );
    assert_eq!(
        reload.apply_boundary(true, false, &BTreeSet::new())[0].current,
        cache.fast_key
    );
    cache.paused.store(false, Ordering::Release);
    wait(&mut reload, false);
    assert!(!reload.is_importing());
    assert_eq!(current(&reload), cache.fast_key);
    reload.policy = ApplyPolicy::OnApproval;
    assert!(
        reload
            .apply_boundary(true, false, &BTreeSet::new())
            .is_empty()
    );
    let changed = reload.apply_boundary(true, false, &BTreeSet::from(["as_color".into()]));
    assert_eq!(changed[0].current, cache.final_key);
    assert_eq!(changed[0].generation, 2);
    assert!(cache.load(cache.fast_key).unwrap().is_some());
    assert!(cache.load(cache.final_key).unwrap().is_some());
    let fast_loads = cache.fast_loads.load(Ordering::Relaxed);
    reload.request("as_color", input);
    assert!(wait(&mut reload, false).iter().any(|e| matches!(
        e,
        ReloadEvent::Ready {
            cache_hit: true,
            ..
        }
    )));
    assert!(!reload.is_importing());
    assert_eq!(cache.fast_loads.load(Ordering::Relaxed), fast_loads);
    assert_eq!(current(&reload), cache.final_key);
}
#[test]
fn hq_failure_and_cancellation_preserve_published_fast_generation() {
    for cancel in [false, true] {
        let (_dir, input, cache, mut reload) = setup(!cancel);
        reload.request("as_color", input);
        wait(&mut reload, false);
        reload.apply_boundary(true, false, &BTreeSet::new());
        assert_eq!(current(&reload), cache.fast_key);
        let start = Instant::now();
        while !cache.entered.load(Ordering::Acquire) {
            assert!(start.elapsed() < Duration::from_secs(15));
            thread::sleep(Duration::from_millis(2));
        }
        if cancel {
            reload.cancel("as_color");
        }
        cache.paused.store(false, Ordering::Release);
        if cancel {
            let start = Instant::now();
            while cache.load(cache.final_key).unwrap().is_none() {
                assert!(start.elapsed() < Duration::from_secs(15));
                thread::sleep(Duration::from_millis(2));
            }
            reload.poll();
        } else {
            wait(&mut reload, true);
        }
        assert!(
            reload
                .apply_boundary(true, false, &BTreeSet::new())
                .is_empty()
        );
        assert_eq!(current(&reload), cache.fast_key);
        assert!(!reload.is_importing());
    }
}
