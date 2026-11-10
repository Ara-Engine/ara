use super::*;

#[test]
fn refresh_rejects_queued_and_inflight_scans_and_acknowledges_equal_successful_rows()
-> ara_ui::Result<()> {
    let (mut browser, mut worker) = scan_fixture::controlled("");
    let rows = vec![Asset::unindexed("A.prefab".into(), "prefab".into())];
    let initial = worker.start();
    worker.publish(initial, rows.clone())?;
    assert!(browser.poll());
    let in_flight = worker.start();
    let barrier = browser.refresh_after_current_scan();
    assert!(worker.requested());
    worker.publish(in_flight, rows.clone())?;
    assert!(!browser.poll(), "An equal snapshot need not rebuild rows");
    assert!(!browser.completed_scan_after(barrier));
    let fresh = worker.start();
    worker.publish(fresh, rows.clone())?;
    assert!(!browser.poll());
    assert!(
        browser.completed_scan_after(barrier),
        "Freshness must advance even for equal rows"
    );
    let barrier = browser.refresh_after_current_scan();
    let failed = worker.start();
    worker.fail(failed)?;
    assert!(browser.poll());
    assert!(
        !browser.completed_scan_after(barrier),
        "A failed scan cannot authorize handoff"
    );
    let retry = worker.start();
    let mut changed = rows;
    changed.push(Asset::unindexed("B.prefab".into(), "prefab".into()));
    worker.publish(retry, changed)?;
    assert!(browser.poll());
    assert!(
        browser.error.is_none(),
        "A changed successful snapshot also clears the earlier error"
    );
    assert!(browser.completed_scan_after(barrier));
    Ok(())
}
