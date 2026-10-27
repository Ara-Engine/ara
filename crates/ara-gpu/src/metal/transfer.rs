//! Copies out of engine images for presentation and shared surfaces, on a classic
//! `MTLCommandQueue` ordered after the device's MTL4 timelines by their shared events.
//! Equal extents and formats blit; anything else goes through MetalPerformanceShaders'
//! bilinear scale, which also converts formats. Both are needed because
//! `CAMetalLayer` has no RGBA8 format and Metal blits neither scale nor convert, like
//! the Vulkan backend's `vkCmdBlitImage`. MPS kernels are prebuilt by Apple, so the
//! backend still compiles no shader source. Classic command buffers make the textures
//! they use resident implicitly, so no residency set is involved.
use super::native::nil;
use super::{MetalDevice, WAIT_TIMEOUT_MS};
use crate::{GpuError, Queue, Result};
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use objc2_metal_performance_shaders::{MPSImageBilinearScale, MPSImageEdgeMode};

/// A native texture borrowed for one transfer.
pub(super) type Texture = ProtocolObject<dyn MTLTexture>;
/// The command buffer a transfer encodes into.
pub(super) type CommandBuffer = ProtocolObject<dyn MTLCommandBuffer>;

/// One classic queue, its completion event and its scale kernel.
pub(super) struct Transfer {
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
    value: u64,
    scale: Retained<MPSImageBilinearScale>,
}

// SAFETY: command queues and MPS kernels may be used from any thread; each transfer
// is encoded, committed and waited inside one `&mut self` call.
unsafe impl Send for Transfer {}
// SAFETY: see `Send`; shared references never encode.
unsafe impl Sync for Transfer {}

impl Transfer {
    /// A transfer queue on `gpu`'s device.
    pub fn new(gpu: &MetalDevice) -> Result<Self> {
        let queue = gpu
            .device
            .newCommandQueue()
            .ok_or_else(|| nil("command queue"))?;
        // SAFETY: a fresh kernel for this device.
        let scale = unsafe {
            MPSImageBilinearScale::initWithDevice(MPSImageBilinearScale::alloc(), &gpu.device)
        };
        // SAFETY: a plain property set on the fresh kernel. Clamping matches the Vulkan
        // blit: edge texels are not blended with transparent black.
        unsafe { scale.setEdgeMode(MPSImageEdgeMode::Clamp) };
        let event = gpu
            .device
            .newSharedEvent()
            .ok_or_else(|| nil("shared event"))?;
        Ok(Self {
            queue,
            event,
            value: 0,
            scale,
        })
    }
    /// Encode with `record` after all of `gpu`'s submitted work, commit and wait.
    pub fn run(
        &mut self,
        gpu: &MetalDevice,
        record: impl FnOnce(&Self, &CommandBuffer) -> Result<()>,
    ) -> Result<()> {
        let cmd = self
            .queue
            .commandBuffer()
            .ok_or_else(|| nil("command buffer"))?;
        for queue in [Queue::Graphics, Queue::Compute] {
            let value = gpu.submitted[queue.index()];
            if value > 0 {
                cmd.encodeWaitForEvent_value(
                    ProtocolObject::from_ref(&*gpu.events[queue.index()]),
                    value,
                );
            }
        }
        record(self, &cmd)?;
        self.value += 1;
        let value = self.value;
        cmd.encodeSignalEvent_value(ProtocolObject::from_ref(&*self.event), value);
        cmd.commit();
        let signalled = gpu.waits.measure(|| {
            self.event
                .waitUntilSignaledValue_timeoutMS(value, WAIT_TIMEOUT_MS)
        });
        if !signalled {
            return Err(GpuError::Backend(anyhow::anyhow!(
                "Metal transfer timed out after {WAIT_TIMEOUT_MS} ms"
            )));
        }
        // The signal is the last command, so completion follows at once.
        cmd.waitUntilCompleted();
        match cmd.status() {
            MTLCommandBufferStatus::Completed => Ok(()),
            status => Err(GpuError::Backend(anyhow::anyhow!(
                "transfer command buffer ended {status:?}: {}",
                cmd.error()
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_default()
            ))),
        }
    }
    /// Copy `from` into `to`: a blit for equal extents and formats, else a bilinear
    /// scale and format conversion over the whole destination.
    pub fn copy(&self, cmd: &CommandBuffer, from: &Texture, to: &Texture) -> Result<()> {
        let same = from.width() == to.width()
            && from.height() == to.height()
            && from.pixelFormat() == to.pixelFormat();
        if same {
            let blit = cmd
                .blitCommandEncoder()
                .ok_or_else(|| nil("blit encoder"))?;
            // SAFETY: equal extents and formats (checked); both textures are live.
            unsafe { blit.copyFromTexture_toTexture(from, to) };
            blit.endEncoding();
        } else {
            // SAFETY: the source is shader-readable (copy sources always are) and the
            // destination shader-writable (presentation and slot textures).
            unsafe {
                self.scale
                    .encodeToCommandBuffer_sourceTexture_destinationTexture(cmd, from, to)
            };
        }
        Ok(())
    }
}
