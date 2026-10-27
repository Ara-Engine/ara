//! A bounded background worker with the Vulkan preparation workers' admission
//! contract: at most two retained requests (one running, one queued or awaiting
//! adoption); `submit` builds its input only when admitted; `poll` and `cancel` never
//! wait. Cancelled results are dropped unseen when they arrive.
use crate::arena::{Arena, Handle};
use crate::{GpuError, Result};
use std::sync::mpsc;

enum Pending<Output> {
    Waiting,
    Cancelled,
    Ready(Result<Output>),
}

/// One worker thread running `Job -> Output`; tickets are `Handle<K>`.
pub(super) struct Worker<K, Job, Output> {
    send: Option<mpsc::Sender<(Handle<K>, Job)>>,
    receive: mpsc::Receiver<(Handle<K>, Result<Output>)>,
    pending: Arena<K, Pending<Output>>,
    count: usize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl<K: 'static, Job: Send + 'static, Output: Send + 'static> Worker<K, Job, Output> {
    pub fn new(name: &str, run: impl Fn(Job) -> Result<Output> + Send + 'static) -> Result<Self> {
        let (send, jobs) = mpsc::channel::<(Handle<K>, Job)>();
        let (results, receive) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for (id, job) in jobs {
                    if results.send((id, run(job))).is_err() {
                        break;
                    }
                }
            })
            .map_err(anyhow::Error::from)?;
        Ok(Self {
            send: Some(send),
            receive,
            pending: Arena::default(),
            count: 0,
            thread: Some(thread),
        })
    }
    fn drain(&mut self) {
        while let Ok((id, result)) = self.receive.try_recv() {
            let Ok(pending) = self.pending.get_mut(id) else {
                continue;
            };
            if matches!(pending, Pending::Cancelled) {
                let _ = self.pending.remove(id);
                self.count -= 1;
            } else {
                *pending = Pending::Ready(result);
            }
        }
    }
    /// Admit a job if fewer than two are retained; None keeps the caller's input.
    pub fn submit(&mut self, build: impl FnOnce() -> Job) -> Result<Option<Handle<K>>> {
        self.drain();
        if self.count == 2 {
            return Ok(None);
        }
        let id = self.pending.insert(Pending::Waiting)?;
        let send = self.send.as_ref().expect("worker alive until drop");
        if send.send((id, build())).is_err() {
            self.pending.remove(id)?;
            return Err(GpuError::Contract("GPU preparation worker stopped".into()));
        }
        self.count += 1;
        Ok(Some(id))
    }
    /// A finished result expires its ticket; None while it is still running.
    pub fn poll(&mut self, id: Handle<K>) -> Result<Option<Result<Output>>> {
        self.drain();
        match self.pending.get(id)? {
            Pending::Waiting => Ok(None),
            Pending::Cancelled => Err(GpuError::StaleHandle),
            Pending::Ready(_) => {
                let Pending::Ready(result) = self.pending.remove(id)? else {
                    unreachable!("ready request")
                };
                self.count -= 1;
                Ok(Some(result))
            }
        }
    }
    /// Expire a ticket immediately; a running job finishes and is dropped unseen.
    pub fn cancel(&mut self, id: Handle<K>) -> Result<()> {
        self.drain();
        match self.pending.get_mut(id)? {
            pending @ Pending::Waiting => *pending = Pending::Cancelled,
            Pending::Cancelled => return Err(GpuError::StaleHandle),
            Pending::Ready(_) => {
                self.pending.remove(id)?;
                self.count -= 1;
            }
        }
        Ok(())
    }
}

impl<K, Job, Output> Drop for Worker<K, Job, Output> {
    fn drop(&mut self) {
        drop(self.send.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
