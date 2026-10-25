use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Source {
    output: CachedImport,
    active: AtomicUsize,
    peak: AtomicUsize,
    reads: AtomicUsize,
    fail: AtomicBool,
}
impl AssetSource for Source {
    fn digest(&self, _: &str) -> Option<Digest> {
        Some(self.output.key)
    }
    fn load(&self, id: &str) -> Result<Option<(Digest, CachedImport)>> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        self.reads.fetch_add(1, Ordering::SeqCst);
        // Vary completion order so errors cannot accidentally depend on it.
        std::thread::sleep(std::time::Duration::from_millis(if id.ends_with('0') {
            3
        } else {
            1
        }));
        let result = if self.fail.load(Ordering::SeqCst) && id.ends_with('7') {
            Err(RuntimeError(format!("pending {id}")))
        } else {
            Ok(Some((self.output.key, self.output.clone())))
        };
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

#[test]
fn cold_preload_is_bounded_ordered_complete_and_keeps_failed_ids_reloadable() -> Result<()> {
    let source = Source {
        output: crate::model::selection_tests::imported(crate::model::selection_tests::scene()),
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        fail: AtomicBool::new(true),
    };
    let ids: BTreeSet<_> = (0..48).map(|i| format!("as_{i:04}")).collect();
    let (mut models, errors) = Models::preload(&source, &ids)?;
    assert_eq!(source.reads.load(Ordering::SeqCst), ids.len());
    assert_eq!(source.active.load(Ordering::SeqCst), 0);
    assert!((1..=workers(ids.len())).contains(&source.peak.load(Ordering::SeqCst)));
    assert!(workers(ids.len()) <= 4);
    assert_eq!(
        models.ids().collect::<Vec<_>>(),
        ids.iter().collect::<Vec<_>>()
    );
    assert_eq!(
        errors.into_iter().map(|error| error.0).collect::<Vec<_>>(),
        ids.iter()
            .filter(|id| id.ends_with('7'))
            .map(|id| format!("pending {id}"))
            .collect::<Vec<_>>()
    );
    for id in &ids {
        assert_eq!(models.get(id).is_none(), id.ends_with('7'));
    }
    assert!(!models.load(&source, "as_0000")?);
    assert_eq!(source.reads.load(Ordering::SeqCst), ids.len());
    source.fail.store(false, Ordering::SeqCst);
    assert!(models.load(&source, "as_0007")?);
    assert!(models.get("as_0007").is_some());
    assert_eq!(source.reads.load(Ordering::SeqCst), ids.len() + 1);
    let (_, errors) = Models::preload(&source, &BTreeSet::new())?;
    assert!(errors.is_empty());
    assert_eq!(source.reads.load(Ordering::SeqCst), ids.len() + 1);
    Ok(())
}
