//! Destruction of completed driver objects, off the frame thread.
use super::context::memory_blocks::{MemoryBlocks, Slice};
use super::{BufferAllocation, ImageAllocation, vk};
use crate::{GpuError, Result};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::JoinHandle;

pub(super) enum Object {
    Buffer(BufferAllocation),
    Image(ImageAllocation),
    Pipeline(vk::Pipeline),
    Memory(vk::DeviceMemory),
    Pool(MemoryBlocks),
}

impl Object {
    fn suballocation(&self) -> Option<Slice> {
        match self {
            Self::Buffer(b) => b.suballocation,
            Self::Image(i) => i.suballocation,
            _ => None,
        }
    }
    pub(super) fn destroy(self, device: &ash::Device) {
        // SAFETY: The device owner admits only objects whose uses on both queue
        // timelines have completed, and joins this worker before device teardown.
        unsafe {
            match self {
                Self::Buffer(buffer) => {
                    device.destroy_buffer(buffer.handle, None);
                    if buffer.suballocation.is_none() {
                        device.free_memory(buffer.memory, None);
                    }
                }
                Self::Image(image) => {
                    device.destroy_image_view(image.view, None);
                    if let Some(storage) = image.storage_view {
                        device.destroy_image_view(storage, None);
                    }
                    device.destroy_image(image.handle, None);
                    if image.suballocation.is_none() {
                        device.free_memory(image.memory, None);
                    }
                }
                Self::Pipeline(pipeline) => device.destroy_pipeline(pipeline, None),
                Self::Memory(memory) => device.free_memory(memory, None),
                Self::Pool(pool) => drop(pool),
            }
        }
    }
}

/// One active batch plus one queued batch. Backpressure retains completed objects
/// on the owner, never blocks a frame, and never delays CPU/descriptor retirement.
pub(super) struct Disposal {
    device: ash::Device,
    send: Option<SyncSender<Vec<Object>>>,
    worker: Option<JoinHandle<()>>,
    pending: Vec<Object>,
    released: Receiver<Slice>,
}

impl Disposal {
    pub fn new(device: ash::Device) -> Result<Self> {
        Self::start(device, || {})
    }

    fn start(device: ash::Device, mut before_batch: impl FnMut() + Send + 'static) -> Result<Self> {
        let (send, receive) = mpsc::sync_channel::<Vec<Object>>(1);
        let (release, released) = mpsc::channel();
        let worker_device = device.clone();
        let worker = std::thread::Builder::new()
            .name("ara-gpu-disposal".into())
            .spawn(move || {
                while let Ok(objects) = receive.recv() {
                    before_batch();
                    crate::cpu_scope!("GPU.destroy_completed");
                    for object in objects {
                        let slice = object.suballocation();
                        object.destroy(&worker_device);
                        if let Some(slice) = slice {
                            let _ = release.send(slice);
                        }
                    }
                }
            })
            .map_err(anyhow::Error::from)?;
        Ok(Self {
            device,
            send: Some(send),
            worker: Some(worker),
            pending: Vec::new(),
            released,
        })
    }

    pub fn released(&self) -> impl Iterator<Item = Slice> + '_ {
        self.released.try_iter()
    }
    pub fn push(&mut self, object: Object) {
        self.pending.push(object);
    }

    pub fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let send = self.send.as_ref().expect("worker remains open until Drop");
        match send.try_send(std::mem::take(&mut self.pending)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(objects)) => {
                self.pending = objects;
                Ok(())
            }
            Err(TrySendError::Disconnected(objects)) => {
                self.pending = objects;
                Err(GpuError::Contract("GPU disposal worker stopped".into()))
            }
        }
    }
}

impl Drop for Disposal {
    fn drop(&mut self) {
        if let Some(send) = self.send.take() {
            if !self.pending.is_empty()
                && let Err(mpsc::SendError(objects)) = send.send(std::mem::take(&mut self.pending))
            {
                for object in objects {
                    object.destroy(&self.device);
                }
            }
            drop(send);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod benchmark;
#[cfg(test)]
mod tests;
