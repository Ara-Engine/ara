//! Owned startup work and nonblocking progress/cancellation, without shared locks.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Progress {
    pub phase: &'static str,
    pub completed: usize,
    pub total: usize,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            phase: "queued",
            completed: 0,
            total: 0,
        }
    }
}

pub(crate) struct Reporter {
    updates: Sender<Progress>,
    cancelled: Arc<AtomicBool>,
}

impl Reporter {
    pub fn check(&self) -> Result<(), &'static str> {
        if self.cancelled.load(Ordering::Acquire) {
            Err("Play start cancelled")
        } else {
            Ok(())
        }
    }

    pub fn report(
        &self,
        phase: &'static str,
        completed: usize,
        total: usize,
    ) -> Result<(), &'static str> {
        self.check()?;
        let _ = self.updates.send(Progress {
            phase,
            completed,
            total,
        });
        Ok(())
    }
}

pub(crate) struct Worker<I, O> {
    pub began: Instant,
    pub progress: Progress,
    updates: Receiver<Progress>,
    cancelled: Arc<AtomicBool>,
    task: JoinHandle<Option<O>>,
    input: Option<Sender<I>>,
}

impl<I: Send + 'static, O: Send + 'static> Worker<I, O> {
    /// Spawn before transferring any live pool/assets. A spawn/send error lets
    /// the service retain that ownership instead of dropping it on the UI thread.
    pub fn spawn(began: Instant, run: fn(I, &Reporter) -> O) -> std::io::Result<Self> {
        let (send, input) = mpsc::channel();
        let (updates, receive) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let reporter = Reporter {
            updates,
            cancelled: Arc::clone(&cancelled),
        };
        let task = thread::Builder::new()
            .name("ara-play-start".into())
            .spawn(move || input.recv().ok().map(|input| run(input, &reporter)))?;
        Ok(Self {
            began,
            progress: Default::default(),
            updates: receive,
            cancelled,
            task,
            input: Some(send),
        })
    }

    pub fn send(&mut self, input: I) -> Result<(), I> {
        self.input
            .take()
            .expect("startup worker accepts one owned input")
            .send(input)
            .map_err(|error| error.0)
    }

    pub fn poll(&mut self) {
        while let Ok(progress) = self.updates.try_recv() {
            self.progress = progress;
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Call only after `finished`; live host ticks never wait for a worker.
    pub fn finish(self) -> thread::Result<Option<O>> {
        self.task.join()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Input {
        gate: Receiver<()>,
        ready: Sender<()>,
    }

    fn held_start(input: Input, reporter: &Reporter) -> Result<(), &'static str> {
        reporter.report("assets", 3, 17)?;
        let _ = input.ready.send(());
        let _ = input.gate.recv_timeout(Duration::from_secs(5));
        reporter.check()
    }

    #[test]
    fn progress_and_cancellation_do_not_wait_for_owned_startup_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let (release, gate) = mpsc::channel();
        let (ready, wait) = mpsc::channel();
        let mut worker = Worker::spawn(Instant::now(), held_start)?;
        worker
            .send(Input { gate, ready })
            .map_err(|_| "startup input rejected")?;
        wait.recv_timeout(Duration::from_secs(5))?;
        worker.poll();
        assert_eq!(
            (
                worker.progress.phase,
                worker.progress.completed,
                worker.progress.total
            ),
            ("assets", 3, 17)
        );
        assert!(!worker.finished(), "the real startup work is still held");
        worker.cancel();
        assert!(worker.cancelled());
        assert!(
            !worker.finished(),
            "cancel must signal without joining the held worker"
        );
        release.send(())?;
        assert_eq!(
            worker.finish().map_err(|_| "startup worker panicked")?,
            Some(Err("Play start cancelled"))
        );
        Ok(())
    }
}
