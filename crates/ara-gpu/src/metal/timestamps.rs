//! GPU timestamps per submission, in the Vulkan backend's layout: index 0 and the
//! last index bracket the whole submission; pass `i` uses `1 + 2i` (begin, after its
//! leading barriers) and `2 + 2i` (end). Both queues share one GPU clock, so mixed-queue
//! timing windows are always available. Heaps are pooled and reused after completion.
use super::native::nil;
use crate::commands::{PassTiming, TimestampSpan};
use crate::{GpuError, Queue, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSRange;
use objc2_metal::*;

/// A timestamp counter heap and its slot count.
pub(super) struct Heap {
    pub heap: Retained<ProtocolObject<dyn MTL4CounterHeap>>,
    capacity: usize,
}

/// One submission's heap and pass labels, resolved after completion.
pub(super) struct Timing {
    pub heap: Heap,
    pub labels: Vec<String>,
}

impl Timing {
    /// Slots this submission writes.
    pub fn slots(&self) -> usize {
        self.labels.len() * 2 + 2
    }
}

/// Free heaps; a submission takes the smallest one that fits.
#[derive(Default)]
pub(super) struct Pool {
    free: Vec<Heap>,
}

impl Pool {
    pub fn take(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        labels: Vec<String>,
    ) -> Result<Timing> {
        let required = labels.len() * 2 + 2;
        let fit = self
            .free
            .iter()
            .enumerate()
            .filter(|(_, heap)| heap.capacity >= required)
            .min_by_key(|(_, heap)| heap.capacity)
            .map(|(index, _)| index);
        let heap = match fit {
            Some(index) => self.free.swap_remove(index),
            None => {
                let capacity = required.next_power_of_two();
                let descriptor = MTL4CounterHeapDescriptor::new();
                descriptor.setType(MTL4CounterHeapType::Timestamp);
                // SAFETY: a nonzero slot count for a timestamp heap.
                unsafe { descriptor.setCount(capacity) };
                let heap = device
                    .newCounterHeapWithDescriptor_error(&descriptor)
                    .map_err(|e| super::native::ns_error("timestamp heap", &e))?;
                Heap { heap, capacity }
            }
        };
        Ok(Timing { heap, labels })
    }
    /// Return a completed submission's heap.
    pub fn recycle(&mut self, heap: Heap) {
        // At most 32 heaps are kept, like the Vulkan query-pool cache.
        if self.free.len() < 32 {
            self.free.push(heap);
        }
    }
}

/// Read a completed submission's timestamps as pass timings and its whole span.
pub(super) fn resolve(
    device: &ProtocolObject<dyn MTLDevice>,
    timing: &Timing,
    queue: Queue,
) -> Result<(Vec<PassTiming>, TimestampSpan)> {
    let slots = timing.slots();
    // SAFETY: the range lies inside the heap and every slot in it was written by the
    // completed submission (begin/end of the submission and of each pass).
    let data = unsafe { timing.heap.heap.resolveCounterRange(NSRange::new(0, slots)) }
        .ok_or_else(|| nil("timestamp resolve"))?;
    let stride = device.sizeOfCounterHeapEntry(MTL4CounterHeapType::Timestamp);
    let bytes = data.to_vec();
    if stride < 8 || bytes.len() < slots * stride {
        return Err(GpuError::Backend(anyhow::anyhow!(
            "timestamp data is {} bytes for {slots} entries of {stride}",
            bytes.len()
        )));
    }
    let scale = 1000.0 / device.queryTimestampFrequency() as f64;
    let ms = |slot: usize| {
        let at = slot * stride;
        let mut tick = [0; 8];
        tick.copy_from_slice(&bytes[at..at + 8]);
        u64::from_le_bytes(tick) as f64 * scale
    };
    let passes = timing
        .labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let (start_ms, end_ms) = (ms(1 + 2 * i), ms(2 + 2 * i));
            PassTiming {
                pass: label.clone(),
                queue,
                milliseconds: (end_ms - start_ms).max(0.),
                start_ms,
                end_ms,
            }
        })
        .collect();
    let span = TimestampSpan {
        start_ms: ms(0),
        end_ms: ms(slots - 1),
    };
    Ok((passes, span))
}
