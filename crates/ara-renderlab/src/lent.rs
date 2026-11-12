//! A value one worker thread works on while its owner carries on: the engine's CPU work for a
//! frame (recording and submitting it, ~25 ms on a large scene) runs there, so the studio's
//! loop keeps handling input and drawing its UI. The value is moved to the worker for a job
//! and moved back when the job is done; nothing is shared, so nothing is locked.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;

type Job<T> = Box<dyn FnOnce(&mut T) + Send>;
type Back<T> = std::thread::Result<Box<T>>;

/// A `T` that is either here, or away on the worker with a job. A shared borrow
/// (`Deref`) needs it here; [`Self::wait`], any `&mut` use ([`DerefMut`](std::ops::DerefMut))
/// waits for the job, and [`Self::try_home`] asks without waiting.
pub(crate) struct Lent<T: Send + 'static> {
    /// `None` while the worker has it (or after its job panicked).
    here: Option<Box<T>>,
    /// A job was sent and its outcome not yet received.
    busy: bool,
    jobs: Option<Sender<(Box<T>, Job<T>)>>,
    back: Receiver<Back<T>>,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Lent<T> {
    /// Starts the worker thread `name`; `value` is here.
    pub fn new(name: &str, value: T) -> std::io::Result<Self> {
        let (jobs, work) = channel::<(Box<T>, Job<T>)>();
        let (done, back) = channel::<Back<T>>();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for (mut value, job) in work {
                    let outcome =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&mut value)));
                    if done.send(outcome.map(|()| value)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            here: Some(Box::new(value)),
            busy: false,
            jobs: Some(jobs),
            back,
            thread: Some(thread),
        })
    }

    /// Hands it to the worker with `job` (waiting first if it is still away).
    pub fn send(&mut self, job: impl FnOnce(&mut T) + Send + 'static) {
        self.wait();
        let value = self.here.take().expect("brought home by `wait`");
        if let Some(jobs) = &self.jobs {
            // The worker's loop ends only when this handle drops its sender.
            jobs.send((value, Box::new(job)))
                .expect("the frame worker is running");
            self.busy = true;
        }
    }

    /// Takes it back if the worker is done with it, and says whether it is here. A job that
    /// panicked panics here, as it would have where it ran.
    pub fn try_home(&mut self) -> bool {
        if self.here.is_none() {
            match self.back.try_recv() {
                Ok(outcome) => {
                    self.busy = false;
                    self.here = Some(settle(outcome));
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => panic!("the frame worker is gone"),
            }
        }
        true
    }

    /// The value, once the worker has handed it back (waiting for the job to finish).
    pub fn wait(&mut self) -> &mut T {
        if self.here.is_none() {
            assert!(self.busy, "the value was lost to a panicking job");
            match self.back.recv() {
                Ok(outcome) => {
                    self.busy = false;
                    self.here = Some(settle(outcome));
                }
                Err(_) => panic!("the frame worker is gone"),
            }
        }
        self.here.as_mut().expect("brought home above")
    }
}

fn settle<T>(outcome: Back<T>) -> Box<T> {
    match outcome {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

impl<T: Send + 'static> std::ops::Deref for Lent<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // A `&self` cannot wait for the worker: callers bring the value home first.
        self.here
            .as_deref()
            .expect("the value is with the frame worker: bring it home first")
    }
}

impl<T: Send + 'static> std::ops::DerefMut for Lent<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.wait()
    }
}

impl<T: Send + 'static> Drop for Lent<T> {
    fn drop(&mut self) {
        // The value comes back first so it drops here, as it did before the worker existed;
        // then the worker's loop ends and it is joined.
        if self.busy {
            drop(self.back.recv());
        }
        drop(self.here.take());
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_runs_on_the_worker_and_the_value_comes_back() {
        let mut lent = Lent::new("test worker", 1_u32).expect("a thread");
        let caller = std::thread::current().id();
        lent.send(move |n| {
            assert_ne!(std::thread::current().id(), caller);
            *n += 1;
        });
        // Using it waits for the job.
        assert_eq!(*lent.wait(), 2);
    }

    #[test]
    fn try_home_does_not_wait() {
        let mut lent = Lent::new("test worker", 0_u32).expect("a thread");
        let (release, gate) = channel::<()>();
        lent.send(move |n| {
            gate.recv().expect("released");
            *n = 7;
        });
        assert!(!lent.try_home());
        release.send(()).expect("the worker waits");
        assert_eq!(*lent.wait(), 7);
        assert!(lent.try_home());
    }

    #[test]
    #[should_panic(expected = "the job broke")]
    fn a_panicking_job_panics_where_the_value_is_used() {
        let mut lent = Lent::new("test worker", 0_u32).expect("a thread");
        lent.send(|_| panic!("the job broke"));
        lent.wait();
    }

    #[test]
    fn dropping_while_away_waits_for_the_job() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = std::sync::Arc::clone(&flag);
        let mut lent = Lent::new("test worker", 0_u32).expect("a thread");
        lent.send(move |_| {
            std::thread::sleep(std::time::Duration::from_millis(30));
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        drop(lent);
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }
}
