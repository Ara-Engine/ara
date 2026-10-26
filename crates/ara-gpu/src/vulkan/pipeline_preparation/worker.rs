use super::*;
use compiler::{Batch, Compiler};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;

enum Pending {
    Waiting,
    Ready(Result<Box<Batch>>),
    Cancelled,
}
pub(in crate::vulkan) struct Worker {
    send: Option<Sender<(PipelinePreparationId, Vec<PipelinePreparation>)>>,
    receive: Receiver<(PipelinePreparationId, Result<Box<Batch>>)>,
    thread: Option<JoinHandle<()>>,
    pending: Arena<PipelinePreparationKind, Pending>,
    count: usize,
}
impl Worker {
    pub fn new(
        device: ash::Device,
        mesh: bool,
        layout: vk::PipelineLayout,
        path: PathBuf,
        info: &DeviceInfo,
    ) -> Result<Self> {
        let (vendor, adapter) = (info.vendor_id, info.device_id);
        let uuid = info.pipeline_cache_uuid.to_vec();
        Self::start(
            move || {
                let seed = std::fs::read(&path).unwrap_or_default();
                let seed = if cache::compatible_identity(&seed, vendor, adapter, &uuid) {
                    seed
                } else {
                    Vec::new()
                };
                Compiler::new(device, mesh, layout, path, seed)
            },
            || {},
        )
    }
    fn start(
        compiler: impl FnOnce() -> Result<Compiler> + Send + 'static,
        mut before_job: impl FnMut() + Send + 'static,
    ) -> Result<Self> {
        let (send, jobs) = mpsc::channel::<(PipelinePreparationId, Vec<PipelinePreparation>)>();
        let (reply, receive) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ara-pipeline-compile".into())
            .spawn(move || {
                let compiler = compiler().map_err(|error| error.to_string());
                while let Ok((id, recipes)) = jobs.recv() {
                    before_job();
                    let result = match &compiler {
                        Ok(compiler) => compiler.compile(recipes),
                        Err(error) => Err(GpuError::Contract(error.to_owned())),
                    };
                    if reply.send((id, result)).is_err() {
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
                    return Err(GpuError::Contract("Pipeline compiler stopped".into()));
                }
            };
            match self.pending.get_mut(id)? {
                pending @ Pending::Waiting => *pending = Pending::Ready(result),
                Pending::Cancelled => {
                    self.pending.remove(id)?;
                    self.count -= 1;
                    if let Ok(batch) = result {
                        batch.dispose(disposal);
                    }
                }
                Pending::Ready(_) => {
                    return Err(GpuError::Contract("Duplicate pipeline result".into()));
                }
            }
        }
        disposal.flush()
    }
    pub fn submit(
        &mut self,
        disposal: &mut disposal::Disposal,
        build: impl FnOnce() -> Vec<PipelinePreparation>,
    ) -> Result<Option<PipelinePreparationId>> {
        self.drain(disposal)?;
        if self.count == 2 {
            return Ok(None);
        }
        let recipes = build();
        if recipes.is_empty() {
            return Err(GpuError::Contract("Empty pipeline preparation".into()));
        }
        let id = self.pending.insert(Pending::Waiting)?;
        if self
            .send
            .as_ref()
            .expect("compiler alive")
            .send((id, recipes))
            .is_err()
        {
            self.pending.remove(id)?;
            return Err(GpuError::Contract("Pipeline compiler stopped".into()));
        }
        self.count += 1;
        Ok(Some(id))
    }
    pub(in crate::vulkan::pipeline_preparation) fn poll(
        &mut self,
        disposal: &mut disposal::Disposal,
        id: PipelinePreparationId,
    ) -> Result<Option<Result<Box<Batch>>>> {
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
    pub fn cancel(
        &mut self,
        disposal: &mut disposal::Disposal,
        id: PipelinePreparationId,
    ) -> Result<()> {
        self.drain(disposal)?;
        match self.pending.get_mut(id)? {
            pending @ Pending::Waiting => *pending = Pending::Cancelled,
            Pending::Cancelled => return Err(GpuError::StaleHandle),
            Pending::Ready(_) => {
                let Pending::Ready(result) = self.pending.remove(id)? else {
                    unreachable!("ready request")
                };
                self.count -= 1;
                if let Ok(batch) = result {
                    batch.dispose(disposal);
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
        // Batch Drop releases unadopted replies before the device/layout dies.
    }
}

#[cfg(test)]
mod tests;
