//! One bounded FIFO writer acknowledges completed flush-to-OS barriers.
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_RECORDS: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(in crate::app::monkey) struct Ticket(u64);
struct Batch {
    ticket: Ticket,
    lines: Vec<String>,
    bytes: usize,
}
struct Completed {
    ticket: Ticket,
    bytes: usize,
    records: usize,
    write: Duration,
    flush: Duration,
    result: io::Result<()>,
}

pub(in crate::app::monkey) struct Writer {
    sender: Option<SyncSender<Batch>>,
    completed: Receiver<Completed>,
    worker: Option<JoinHandle<()>>,
    lines: Vec<String>,
    bytes: usize,
    queued: Option<Batch>,
    next: u64,
    acknowledged: u64,
    submitted: u64,
    records: u64,
    written_bytes: u64,
    backpressure: u64,
    flushed_records: u64,
    flushed_bytes: u64,
    write_ms: f64,
    flush_ms: f64,
    max_write_ms: f64,
    max_flush_ms: f64,
    rejected_records: u64,
    error: Option<String>,
    error_count: u64,
    final_drained: bool,
}
impl Writer {
    pub fn new(path: &Path) -> io::Result<Self> {
        Self::spawn(BufWriter::new(File::create(path)?))
    }
    fn spawn(mut output: impl Write + Send + 'static) -> io::Result<Self> {
        let (sender, batches) = mpsc::sync_channel::<Batch>(1);
        let (done, completed) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("editor-monkey-journal".into())
            .spawn(move || {
                while let Ok(batch) = batches.recv() {
                    let at = Instant::now();
                    let result: io::Result<()> = (|| {
                        for line in &batch.lines {
                            output.write_all(line.as_bytes())?;
                            output.write_all(b"\n")?;
                        }
                        Ok(())
                    })();
                    let write = at.elapsed();
                    let at = Instant::now();
                    // Preserve the existing contract: BufWriter.flush reaches OS
                    // buffers. This is deliberately not sync_data/fsync durability.
                    let flush_result = output.flush();
                    let flush = at.elapsed();
                    let _ = done.send(Completed {
                        ticket: batch.ticket,
                        bytes: batch.bytes,
                        records: batch.lines.len(),
                        write,
                        flush,
                        result: result.and(flush_result),
                    });
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            completed,
            worker: Some(worker),
            lines: Vec::new(),
            bytes: 0,
            queued: None,
            next: 0,
            acknowledged: 0,
            submitted: 0,
            records: 0,
            written_bytes: 0,
            backpressure: 0,
            flushed_records: 0,
            flushed_bytes: 0,
            write_ms: 0.,
            flush_ms: 0.,
            max_write_ms: 0.,
            max_flush_ms: 0.,
            rejected_records: 0,
            error: None,
            error_count: 0,
            final_drained: false,
        })
    }

    #[cfg(test)]
    pub(in crate::app::monkey) fn test_sink(
        output: impl Write + Send + 'static,
    ) -> io::Result<Self> {
        Self::spawn(output)
    }

    /// Metadata is retained in order until the next begin barrier or final drain.
    /// Reaching the cap is an explicit evidence failure, never silent truncation.
    pub fn record(&mut self, line: String) -> io::Result<()> {
        self.collect();
        if let Err(error) = self.check() {
            self.rejected_records += 1;
            return Err(error);
        }
        let bytes = self
            .bytes
            .checked_add(line.len())
            .and_then(|n| n.checked_add(1));
        if bytes.is_none_or(|n| n > MAX_BYTES) || self.lines.len() >= MAX_RECORDS {
            self.fail("Monkey journal metadata backlog exceeded its bounded capacity".into());
            self.rejected_records += 1;
            return self.check();
        }
        self.bytes = bytes.unwrap_or(MAX_BYTES);
        self.lines.push(line);
        Ok(())
    }

    pub fn begin(&mut self, line: String) -> io::Result<Ticket> {
        if self.queued.is_some() {
            return Err(io::Error::other("A journal barrier is already queued"));
        }
        self.record(line)?;
        let batch = self.batch();
        let ticket = batch.ticket;
        self.queued = Some(batch);
        self.submit()?;
        Ok(ticket)
    }
    pub fn next_ticket(&self) -> Ticket {
        Ticket(self.next + 1)
    }
    pub fn poll(&mut self, ticket: Ticket) -> io::Result<bool> {
        self.collect();
        self.check()?;
        self.submit()?;
        Ok(self.acknowledged >= ticket.0)
    }
    fn batch(&mut self) -> Batch {
        self.next += 1;
        Batch {
            ticket: Ticket(self.next),
            lines: std::mem::take(&mut self.lines),
            bytes: std::mem::take(&mut self.bytes),
        }
    }
    fn submit(&mut self) -> io::Result<()> {
        let Some(batch) = self.queued.take() else {
            return Ok(());
        };
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| io::Error::other("Journal writer is closed"))?;
        match sender.try_send(batch) {
            Ok(()) => self.submitted += 1,
            Err(TrySendError::Full(batch)) => {
                self.backpressure += 1;
                self.queued = Some(batch);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.fail("Monkey journal writer disconnected".into());
            }
        }
        self.check()
    }
    fn collect(&mut self) {
        while let Ok(done) = self.completed.try_recv() {
            self.acknowledged = self.acknowledged.max(done.ticket.0);
            self.records += done.records as u64;
            self.written_bytes += done.bytes as u64;
            if done.result.is_ok() {
                self.flushed_records += done.records as u64;
                self.flushed_bytes += done.bytes as u64;
            }
            let write = done.write.as_secs_f64() * 1000.;
            let flush = done.flush.as_secs_f64() * 1000.;
            self.write_ms += write;
            self.flush_ms += flush;
            self.max_write_ms = self.max_write_ms.max(write);
            self.max_flush_ms = self.max_flush_ms.max(flush);
            if let Err(error) = done.result {
                self.fail(error.to_string());
            }
        }
    }
    pub fn fail(&mut self, error: String) {
        self.error_count += 1;
        self.error.get_or_insert(error);
    }
    fn check(&self) -> io::Result<()> {
        match &self.error {
            Some(error) => Err(io::Error::other(error.as_str())),
            None => Ok(()),
        }
    }

    /// Called only after the native frame loop: flush every terminal record, close
    /// the FIFO and join its owned worker before constructing the final report.
    pub fn finish(&mut self) -> io::Result<()> {
        if let Some(batch) = self.queued.take() {
            self.send_final(batch);
        }
        if !self.lines.is_empty() {
            let batch = self.batch();
            self.send_final(batch);
        }
        self.sender.take();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            self.fail("Monkey journal worker panicked".into());
        }
        self.collect();
        self.final_drained = self.acknowledged == self.next && self.error.is_none();
        self.check()
    }
    fn send_final(&mut self, batch: Batch) {
        match self.sender.as_ref().map(|sender| sender.send(batch)) {
            Some(Ok(())) => self.submitted += 1,
            _ => self.fail("Cannot drain the monkey journal writer".into()),
        }
    }
    pub fn report(&mut self) -> Value {
        self.collect();
        json!({"submittedBatches":self.submitted,"completedTicket":self.acknowledged,"lastTicket":self.next,
            "recordsAttempted":self.records,"bytesAttempted":self.written_bytes,"recordsFlushed":self.flushed_records,"bytesFlushed":self.flushed_bytes,"queuedForSubmission":self.queued.is_some(),
            "metadataRecordsPending":self.lines.len(),"metadataBytesPending":self.bytes,"backpressurePolls":self.backpressure,
            "workerWriteMs":self.write_ms,"workerFlushMs":self.flush_ms,"maxWorkerWriteMs":self.max_write_ms,"maxWorkerFlushMs":self.max_flush_ms,
            "error":self.error,"errorCount":self.error_count,"finalDrained":self.final_drained,
            "recordsNotQueued":self.rejected_records,
            "queueCapacity":1,"maxBatchBytes":MAX_BYTES,"maxBatchRecords":MAX_RECORDS,
            "acknowledgementScope":"Completed writes and BufWriter.flush to OS buffers before dispatch; no sync_data/fsync or power-loss guarantee"})
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
