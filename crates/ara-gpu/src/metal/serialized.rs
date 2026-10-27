//! Serialized native submissions on the graphics timeline, for the paths that copy
//! out of engine images outside a portable command list: presentation and the UI
//! pixel readback. One compute encoder is ordered after all work already submitted on
//! both queues, committed, signalled and retired like a portable submission; callers
//! wait for the returned point, as the Vulkan backend's serialized v0 paths do.
use super::MetalDevice;
use super::native::{nil, pool};
use crate::{Device, GpuError, Image, Queue, Resource, Result, TimelinePoint};
use ash::vk;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use std::ptr::NonNull;

type Encoder = ProtocolObject<dyn MTL4ComputeCommandEncoder>;
type NativeQueue = ProtocolObject<dyn MTL4CommandQueue>;

impl MetalDevice {
    /// Record blits with `encode`, then commit on the graphics queue. `around` runs
    /// on the native queue just before (`true`) and after (`false`) the commit, for
    /// drawable waits and signals.
    pub(super) fn submit_serialized(
        &mut self,
        encode: impl FnOnce(&Encoder),
        mut around: impl FnMut(&NativeQueue, bool),
    ) -> Result<TimelinePoint> {
        let queue = Queue::Graphics;
        let point = TimelinePoint {
            queue,
            value: self.submitted[queue.index()]
                .checked_add(1)
                .ok_or_else(|| GpuError::Contract("Queue timeline exhausted".into()))?,
        };
        let allocator = match self.allocators.pop() {
            Some(allocator) => allocator,
            None => self
                .device
                .newCommandAllocator()
                .ok_or_else(|| nil("command allocator"))?,
        };
        let cmd = self
            .device
            .newCommandBuffer()
            .ok_or_else(|| nil("command buffer"))?;
        cmd.beginCommandBufferWithAllocator(&allocator);
        let Some(encoder) = cmd.computeCommandEncoder() else {
            cmd.endCommandBuffer();
            allocator.reset();
            self.allocators.push(allocator);
            return Err(nil("compute encoder"));
        };
        // Every earlier write on this queue precedes the copies.
        encoder.barrierAfterQueueStages_beforeStages_visibilityOptions(
            MTLStages::All,
            MTLStages::Blit,
            MTL4VisibilityOptions::Device,
        );
        encode(&encoder);
        encoder.endEncoding();
        cmd.endCommandBuffer();
        if std::mem::take(&mut self.residency_dirty) {
            self.residency.commit();
        }
        let native = &self.queues[queue.index()];
        // Sources may have been written on the compute queue too.
        for other in [Queue::Graphics, Queue::Compute] {
            let value = self.submitted[other.index()];
            if value > 0 {
                native.waitForEvent_value(
                    ProtocolObject::from_ref(&*self.events[other.index()]),
                    value,
                );
            }
        }
        around(native, true);
        let buffers = [NonNull::from(&*cmd)];
        // SAFETY: one ended, never-committed command buffer; the array outlives the call.
        unsafe { native.commit_count(NonNull::from(&buffers[0]), 1) };
        around(native, false);
        native.signalEvent_value(
            ProtocolObject::from_ref(&*self.events[queue.index()]),
            point.value,
        );
        self.submitted[queue.index()] = point.value;
        self.pending.push(super::Pending {
            point,
            allocator,
            chunks: Vec::new(),
            timing: None,
        });
        Ok(point)
    }

    /// Read one `R32_SFLOAT` texel of a completed image (the editor's ID pick), the
    /// Metal form of the Vulkan backend's UI-queue readback.
    ///
    /// # Safety
    /// The same contract as the Vulkan backend's `read_ui_pixel`: `image` is
    /// completed and stays immutable through the call. Metal has no image layouts or
    /// queue ownership, so nothing else is required.
    pub unsafe fn read_ui_pixel(&mut self, image: Image, pixel: [u32; 2]) -> Result<[u8; 4]> {
        pool(|| {
            let raw = &self.images.get(image)?.raw;
            if raw.info.pixel != MTLPixelFormat::R32Float
                || pixel[0] >= raw.size[0]
                || pixel[1] >= raw.size[1]
                || !raw.usage.contains(vk::ImageUsageFlags::TRANSFER_SRC)
            {
                return Err(GpuError::Contract("Invalid presented ID pixel".into()));
            }
            let texture = raw.texture.clone();
            let readback = self.create_readback_buffer(4)?;
            let target = self.buffers.get(readback)?.raw.buffer.clone();
            let copied = self.submit_serialized(
                |encoder| {
                    let origin = MTLOrigin {
                        x: pixel[0] as usize,
                        y: pixel[1] as usize,
                        z: 0,
                    };
                    let one = MTLSize {
                        width: 1,
                        height: 1,
                        depth: 1,
                    };
                    // SAFETY: one in-bounds texel (checked) of a resident R32 texture
                    // into a resident 4-byte buffer.
                    unsafe {
                        encoder.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                            &texture, 0, 0, origin, one, &target, 0, 4, 4,
                        );
                    }
                },
                |_, _| {},
            );
            let texel = copied
                .and_then(|point| Device::wait(self, point))
                .and_then(|()| self.read(readback));
            Device::retire(self, Resource::Buffer(readback))?;
            texel?
                .try_into()
                .map_err(|_| GpuError::Contract("ID readback returned a short buffer".into()))
        })
    }
}
