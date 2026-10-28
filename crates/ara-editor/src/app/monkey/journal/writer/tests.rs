//! Blocking/failing storage is synchronized by channels, never timing sleeps.
use super::*;

struct Sink {
    bytes: Vec<u8>,
    first: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
    written: mpsc::Sender<String>,
    fail_write: bool,
    fail_flush: bool,
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            return Err(io::Error::other("injected journal write failure"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if let Some((entered, release)) = self.first.take() {
            let _ = entered.send(());
            release.recv().map_err(io::Error::other)?;
        }
        if self.fail_flush {
            return Err(io::Error::other("injected journal flush failure"));
        }
        self.written
            .send(String::from_utf8(std::mem::take(&mut self.bytes)).map_err(io::Error::other)?)
            .map_err(io::Error::other)
    }
}
struct Release(Option<mpsc::Sender<()>>);
impl Release {
    fn open(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.open();
    }
}
fn acknowledge(writer: &mut Writer, ticket: Ticket) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !writer.poll(ticket)? {
        if Instant::now() >= deadline {
            return Err(io::Error::other("test journal did not acknowledge"));
        }
        thread::yield_now();
    }
    Ok(())
}

#[test]
fn begin_ack_waits_for_flush_and_terminal_metadata_remains_fifo() -> io::Result<()> {
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (written, observed) = mpsc::channel();
    let mut writer = Writer::spawn(Sink {
        bytes: Vec::new(),
        first: Some((entered, wait)),
        written,
        fail_write: false,
        fail_flush: false,
    })?;
    let mut gate = Release(Some(release));
    writer.record("metadata-before".into())?;
    let first = writer.begin("begin-first".into())?;
    started
        .recv_timeout(Duration::from_secs(5))
        .map_err(io::Error::other)?;
    assert!(!writer.poll(first)?);
    writer.record("end-first".into())?;
    gate.open();
    acknowledge(&mut writer, first)?;
    let second = writer.begin("begin-second".into())?;
    acknowledge(&mut writer, second)?;
    writer.record("end-second".into())?;
    writer.finish()?;
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        [
            "metadata-before\nbegin-first\n",
            "end-first\nbegin-second\n",
            "end-second\n"
        ]
    );
    assert_eq!(writer.report()["finalDrained"], true);
    Ok(())
}

#[test]
fn bounded_queue_keeps_the_unsent_barrier_instead_of_dropping_or_blocking() -> io::Result<()> {
    let (entered, started) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (written, observed) = mpsc::channel();
    let mut writer = Writer::spawn(Sink {
        bytes: Vec::new(),
        first: Some((entered, wait)),
        written,
        fail_write: false,
        fail_flush: false,
    })?;
    let mut gate = Release(Some(release));
    let first = writer.begin("first".into())?;
    started
        .recv_timeout(Duration::from_secs(5))
        .map_err(io::Error::other)?;
    let _second = writer.begin("second".into())?;
    let third = writer.begin("third".into())?;
    assert!(!writer.poll(third)?);
    assert_eq!(writer.report()["queuedForSubmission"], true);
    assert!(!writer.poll(first)?);
    gate.open();
    acknowledge(&mut writer, third)?;
    writer.finish()?;
    assert_eq!(
        observed.try_iter().collect::<Vec<_>>(),
        ["first\n", "second\n", "third\n"]
    );
    Ok(())
}

#[test]
fn write_and_flush_failures_never_acknowledge_success_and_survive_final_drain() {
    for (fail_write, fail_flush) in [(true, false), (false, true)] {
        let (written, _observed) = mpsc::channel();
        let mut writer = Writer::spawn(Sink {
            bytes: Vec::new(),
            first: None,
            written,
            fail_write,
            fail_flush,
        })
        .unwrap();
        let ticket = writer.begin("begin".into()).unwrap();
        assert!(acknowledge(&mut writer, ticket).is_err());
        assert!(writer.finish().is_err());
        assert_eq!(writer.report()["finalDrained"], false);
        assert!(writer.report()["error"].is_string());
    }
}

#[test]
fn a_metadata_capacity_failure_is_explicit_instead_of_truncating_evidence() {
    let (written, _observed) = mpsc::channel();
    let mut writer = Writer::spawn(Sink {
        bytes: Vec::new(),
        first: None,
        written,
        fail_write: false,
        fail_flush: false,
    })
    .unwrap();
    assert!(writer.record("x".repeat(MAX_BYTES + 1)).is_err());
    assert!(writer.finish().is_err());
    assert_eq!(writer.report()["recordsNotQueued"], 1);
    assert_eq!(writer.report()["finalDrained"], false);
}
