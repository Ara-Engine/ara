//! Exercise blocked and failed storage without sleeping or touching the profile.
use super::*;

#[test]
fn blocked_checkpoint_storage_never_blocks_submission_and_final_write_is_last() -> io::Result<()> {
    let (started, began) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (written, observed) = mpsc::channel();
    let mut first = true;
    let mut reports = Reports::spawn(PathBuf::new(), move |_, text| {
        if std::mem::take(&mut first) {
            let _ = started.send(());
            wait.recv().map_err(io::Error::other)?;
        }
        written.send(text.to_owned()).map_err(io::Error::other)
    })?;
    reports.checkpoint("first".into())?;
    began
        .recv_timeout(Duration::from_secs(5))
        .map_err(io::Error::other)?;
    let (submitted, done) = mpsc::channel();
    let caller = thread::spawn(move || {
        let result = (|| {
            reports.checkpoint("waiting".into())?;
            reports.checkpoint("coalesced".into())?;
            Ok::<_, io::Error>(reports.report())
        })();
        let _ = submitted.send(result);
        reports
    });
    let result = done.recv_timeout(Duration::from_secs(2));
    // Always unblock the owned worker before asserting, including a broken implementation.
    release.send(()).map_err(io::Error::other)?;
    let mut reports = caller
        .join()
        .map_err(|_| io::Error::other("Submission thread panicked"))?;
    let report = result.map_err(io::Error::other)??;
    assert_eq!(report["coalescedCheckpoints"], 1);
    reports.finish("authoritative final".into())?;
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        ["first", "waiting", "authoritative final"]
    );
    assert_eq!(reports.report()["completedWrites"], 3);
    Ok(())
}

#[test]
fn background_errors_remain_failures_even_when_the_final_write_succeeds() -> io::Result<()> {
    let mut first = true;
    let (written, observed) = mpsc::channel();
    let mut reports = Reports::spawn(PathBuf::new(), move |_, text| {
        if std::mem::take(&mut first) {
            Err(io::Error::other("injected checkpoint storage failure"))
        } else {
            written.send(text.to_owned()).map_err(io::Error::other)
        }
    })?;
    reports.checkpoint("first".into())?;
    let error = reports
        .finish("final".into())
        .expect_err("IO failures cannot pass");
    assert!(
        error
            .to_string()
            .contains("injected checkpoint storage failure")
    );
    assert_eq!(observed.try_iter().collect::<Vec<_>>(), ["final"]);
    assert_eq!(reports.report()["completedWrites"], 2);
    Ok(())
}
