//! Per-submission bump allocation for top-level argument buffers and ordered buffer
//! updates. Chunks are shared buffers that stay resident for the device's lifetime and
//! return to the pool only after the submission that wrote them completes, so the CPU
//! never overwrites memory the GPU may still read.
use super::native::nil;
use crate::Result;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLBuffer, MTLDevice, MTLResourceOptions};

/// Chunk size; one chunk holds 4096 top-level buffers or a maximal 64 KiB update.
const CHUNK_BYTES: u64 = 256 * 1024;

/// A pooled shared buffer.
pub(super) type Chunk = Retained<ProtocolObject<dyn MTLBuffer>>;

/// Free chunks, reused once their submission completed.
#[derive(Default)]
pub(super) struct Pool {
    free: Vec<Chunk>,
}

impl Pool {
    /// Take a free chunk, or create one; `created` reports chunks that need residency.
    fn take(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        created: &mut Vec<Chunk>,
        size: u64,
    ) -> Result<Chunk> {
        if let Some(index) = self
            .free
            .iter()
            .position(|chunk| chunk.length() as u64 >= size)
        {
            return Ok(self.free.swap_remove(index));
        }
        let chunk = device
            .newBufferWithLength_options(
                size.max(CHUNK_BYTES) as usize,
                MTLResourceOptions::StorageModeShared,
            )
            .ok_or_else(|| nil("scratch chunk"))?;
        created.push(chunk.clone());
        Ok(chunk)
    }
    /// Return the chunks of a completed submission.
    pub fn recycle(&mut self, chunks: Vec<Chunk>) {
        self.free.extend(chunks);
    }
}

/// The bump allocator of one submission.
#[derive(Default)]
pub(super) struct Scratch {
    pub chunks: Vec<Chunk>,
    offset: u64,
}

impl Scratch {
    /// Copy `bytes` into scratch memory, 8-byte aligned (the converter's requirement
    /// for argument-buffer addresses), and return the chunk and byte offset.
    pub fn write(
        &mut self,
        pool: &mut Pool,
        device: &ProtocolObject<dyn MTLDevice>,
        created: &mut Vec<Chunk>,
        bytes: &[u8],
    ) -> Result<(Chunk, u64)> {
        let size = bytes.len() as u64;
        let aligned = self.offset.next_multiple_of(8);
        if self.chunks.is_empty()
            || aligned + size > self.chunks.last().map_or(0, |c| c.length() as u64)
        {
            self.chunks.push(pool.take(device, created, size)?);
            self.offset = 0;
        }
        let offset = self.offset.next_multiple_of(8);
        let chunk = self
            .chunks
            .last()
            .expect("a chunk was just ensured")
            .clone();
        // SAFETY: `offset + size <= chunk.length()` lies inside this shared chunk. The chunk
        // belongs to this unsubmitted recording (fresh, or recycled after its previous
        // submission completed), so no GPU work reads these bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                chunk.contents().cast::<u8>().as_ptr().add(offset as usize),
                bytes.len(),
            );
        }
        self.offset = offset + size;
        Ok((chunk, offset))
    }
}
