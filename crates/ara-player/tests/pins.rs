//! Play pins only generations a player process can load. On a fresh project the first
//! `pins` must already be on disk (players read the cache from another process) and
//! must be the final preset: a progressive preview is published before it is
//! persisted, and a later pass replaces it.
#![allow(clippy::unwrap_used)]
mod support;

use ara_asset::{CacheBackend, Digest, FileCache};
use ara_play_host::assets::Assets;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
fn first_pins_are_persisted_final_generations() {
    let dir = support::demo_copy("pins");
    let worker = support::import_worker(&PathBuf::from(env!("CARGO_BIN_EXE_ara-player")));
    let mut assets = Assets::new(&dir, &worker).unwrap();
    let wanted = BTreeSet::from(["as_fox".to_owned()]);
    let pins = assets.pins(&wanted, Duration::from_secs(180)).unwrap();
    let pinned = pins["as_fox"].clone();

    // What a player does with the snapshot: open the cache directory and load.
    let cache = FileCache::at(&assets.cache).unwrap();
    let digest = Digest(ara_play::unhex(&pinned).unwrap());
    assert!(
        cache.load(digest).unwrap().is_some(),
        "pinned generation {pinned} is not in the disk cache yet"
    );

    let settled = Instant::now();
    while assets.importing() {
        assert!(
            settled.elapsed() < Duration::from_secs(180),
            "imports never settled"
        );
        assets.poll(false, true);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        assets.digest("as_fox").as_deref(),
        Some(pinned.as_str()),
        "a later pass replaced the generation Play pinned"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
