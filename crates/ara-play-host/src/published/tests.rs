//! The feed has one owner: a reader attaching late sees its current state, a second
//! pipeline on the project publishes nothing while the owner lives, and a write a
//! reader's lock deferred lands on the owner's next tick.
use super::*;

/// A fresh project root under `target/` (tests never write tracked files).
fn root(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-play-host/feed")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch root");
    root
}

fn publisher(root: &Path) -> Publisher {
    Publisher::new(root, FileCache::new(root).expect("cache")).expect("publisher")
}

fn pending(ids: &[&str]) -> ImportProgress {
    ImportProgress {
        total: ids.len(),
        pending: ids.iter().map(|id| (*id).to_owned()).collect(),
        ..Default::default()
    }
}

#[test]
fn a_reader_attaching_late_reads_the_current_progress_and_snapshot() {
    let root = root("late");
    let mut owner = publisher(&root);
    owner.progress(pending(&["as_a", "as_b"]));
    let late = PublishedAssets::for_project(&root).expect("attach");
    assert_eq!(late.progress_revision(), 1);
    assert_eq!(
        late.progress().as_deref(),
        Some(&pending(&["as_a", "as_b"]))
    );
    assert!(late.revision() > 0 && late.snapshot().is_some());
}

#[test]
fn a_second_pipeline_publishes_nothing_until_the_owner_is_dropped() {
    let root = root("second");
    let reader = PublishedAssets::for_project(&root).expect("attach");
    let mut owner = publisher(&root);
    let done = ImportProgress {
        done: 2,
        total: 2,
        ..Default::default()
    };
    owner.progress(done.clone());
    let (revision, progressed) = (reader.revision(), reader.progress_revision());
    // Export packaging's own pipeline starts from scratch: every source pending.
    let mut export = publisher(&root);
    export.progress(pending(&["as_a", "as_b"]));
    export.tick();
    assert_eq!(
        (reader.revision(), reader.progress_revision()),
        (revision, progressed)
    );
    assert_eq!(reader.progress().as_deref(), Some(&done));
    // With the owner gone the next pipeline takes over; its snapshot revision is
    // above every earlier one, though it started from the same snapshot.
    drop(owner);
    export.tick();
    assert!(reader.revision() > revision);
    assert!(reader.progress_revision() > progressed);
    assert_eq!(
        reader.progress().as_deref(),
        Some(&pending(&["as_a", "as_b"]))
    );
}

#[test]
fn a_write_a_reader_held_back_lands_on_the_next_tick() {
    let root = root("deferred");
    let reader = PublishedAssets::for_project(&root).expect("attach");
    let mut owner = publisher(&root);
    {
        // A reader mid-read holds the lock the write needs; the owner never waits.
        let _reading = reader.shared.progress.read().expect("progress lock");
        owner.progress(pending(&["as_a"]));
        assert_eq!(reader.progress_revision(), 0);
    }
    owner.tick();
    assert_eq!(reader.progress_revision(), 1);
    assert_eq!(reader.progress().as_deref(), Some(&pending(&["as_a"])));
}
