use super::*;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

enum Pending {
    Waiting,
    Ready(Result<Box<raw::Raw>>),
    Cancelled,
}

pub(in crate::vulkan) struct Worker {
    send: Option<Sender<(PreparationId, ResourcePreparation)>>,
    receive: Receiver<(PreparationId, Result<Box<raw::Raw>>)>,
    thread: Option<JoinHandle<()>>,
    pending: Arena<PreparationKind, Pending>,
    count: usize,
}

impl Worker {
    pub fn new(allocator: allocation::Allocator) -> Result<Self> {
        Self::start(allocator, || {})
    }

    fn start(
        allocator: allocation::Allocator,
        mut before_job: impl FnMut() + Send + 'static,
    ) -> Result<Self> {
        let (send, jobs) = mpsc::channel::<(PreparationId, ResourcePreparation)>();
        let (reply, receive) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ara-gpu-prepare".into())
            .spawn(move || {
                while let Ok((id, request)) = jobs.recv() {
                    before_job();
                    if reply
                        .send((id, raw::Raw::create(&allocator, request)))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(anyhow::Error::from)?;
        Ok(Self {
            send: Some(send),
            receive,
            thread: Some(thread),
            pending: Arena::default(),
            count: 0,
        })
    }

    pub(in crate::vulkan) fn drain(&mut self, disposal: &mut disposal::Disposal) -> Result<()> {
        loop {
            let (id, result) = match self.receive.try_recv() {
                Ok(reply) => reply,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err(GpuError::Contract("GPU preparation worker stopped".into()));
                }
            };
            match self.pending.get_mut(id)? {
                pending @ Pending::Waiting => {
                    *pending = Pending::Ready(result);
                }
                Pending::Cancelled => {
                    self.pending.remove(id)?;
                    self.count -= 1;
                    if let Ok(raw) = result {
                        raw.dispose(disposal);
                    }
                }
                Pending::Ready(_) => {
                    return Err(GpuError::Contract("Duplicate preparation result".into()));
                }
            }
        }
        disposal.flush()
    }

    pub fn submit(
        &mut self,
        disposal: &mut disposal::Disposal,
        build: impl FnOnce() -> ResourcePreparation,
    ) -> Result<Option<PreparationId>> {
        self.drain(disposal)?;
        if self.count == 2 {
            return Ok(None);
        }
        let request = build();
        let id = self.pending.insert(Pending::Waiting)?;
        let send = self.send.as_ref().expect("preparation worker alive");
        if send.send((id, request)).is_err() {
            self.pending.remove(id)?;
            return Err(GpuError::Contract("GPU preparation worker stopped".into()));
        }
        self.count += 1;
        Ok(Some(id))
    }

    pub fn poll(
        &mut self,
        disposal: &mut disposal::Disposal,
        id: PreparationId,
    ) -> Result<Option<Result<Box<raw::Raw>>>> {
        self.drain(disposal)?;
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

    pub fn cancel(&mut self, disposal: &mut disposal::Disposal, id: PreparationId) -> Result<()> {
        self.drain(disposal)?;
        match self.pending.get_mut(id)? {
            pending @ Pending::Waiting => {
                *pending = Pending::Cancelled;
            }
            Pending::Cancelled => return Err(GpuError::StaleHandle),
            Pending::Ready(_) => {
                let Pending::Ready(result) = self.pending.remove(id)? else {
                    unreachable!("ready request")
                };
                self.count -= 1;
                if let Ok(raw) = result {
                    raw.dispose(disposal);
                }
            }
        }
        disposal.flush()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        drop(self.send.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // Replies and ready state own unadopted native allocations; explicit
        // device teardown drops them before the creating device disappears.
    }
}

#[cfg(test)]
mod tests;
