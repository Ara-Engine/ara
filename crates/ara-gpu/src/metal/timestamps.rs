//! GPU timestamps per submission, in the Vulkan backend's layout: index 0 and the
//! last index bracket the whole submission; pass `i` uses `1 + 2i` (begin, after its
//! leading barriers) and `2 + 2i` (end), all relative to the submission's first slot.
//! Both queues share one GPU clock, so mixed-queue timing windows are always available.
//! Metal allows only 32 counter heaps per process, so each device owns one heap of the
//! maximum size and in-flight submissions share it as slot ranges, freed on completion.
//! A submission that finds no free range (or no heap) runs untimed.
use super::native::nil;
use crate::commands::{PassTiming, TimestampSpan};
use crate::{GpuError, Queue, Result};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSRange;
use objc2_metal::*;

/// Slots of a device's timestamp heap: the largest Metal allows.
pub(super) const SLOTS: usize = 4096;

/// One submission's slot range and pass labels, resolved after completion.
pub(super) struct Timing {
    /// The device's heap (a retain of the pool's), for encoding and resolving.
    pub heap: Retained<ProtocolObject<dyn MTL4CounterHeap>>,
    /// First slot of the submission's range.
    pub base: usize,
    pub labels: Vec<String>,
    /// Slots never written because they measure the same point as an earlier one,
    /// as `(slot, written slot)` relative to `base`; resolved as copies.
    pub aliases: Vec<(usize, usize)>,
}

impl Timing {
    /// Slots this submission writes.
    pub fn slots(&self) -> usize {
        self.labels.len() * 2 + 2
    }
}

/// A device's one timestamp heap, created on first use, and its in-flight ranges.
#[derive(Default)]
pub(super) struct Pool {
    heap: Option<Retained<ProtocolObject<dyn MTL4CounterHeap>>>,
    /// `(first slot, slots)` of in-flight submissions, sorted by first slot.
    used: Vec<(usize, usize)>,
}

impl Pool {
    /// The first free range for a submission with these passes, or `None` (the
    /// submission runs untimed) when none fits or the process is out of heaps.
    pub fn take(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        labels: Vec<String>,
    ) -> Option<Timing> {
        let required = labels.len() * 2 + 2;
        let heap = match &self.heap {
            Some(heap) => heap.clone(),
            None => {
                let descriptor = MTL4CounterHeapDescriptor::new();
                descriptor.setType(MTL4CounterHeapType::Timestamp);
                // SAFETY: a nonzero slot count within Metal's timestamp heap maximum.
                unsafe { descriptor.setCount(SLOTS) };
                let heap = device
                    .newCounterHeapWithDescriptor_error(&descriptor)
                    .ok()?;
                self.heap.insert(heap).clone()
            }
        };
        let mut base = 0;
        let mut at = self.used.len();
        for (index, &(first, slots)) in self.used.iter().enumerate() {
            if first - base >= required {
                at = index;
                break;
            }
            base = first + slots;
        }
        if at == self.used.len() && SLOTS - base < required {
            return None;
        }
        self.used.insert(at, (base, required));
        Some(Timing {
            heap,
            base,
            labels,
            aliases: Vec::new(),
        })
    }
    /// Free a completed (or never committed) submission's range.
    pub fn recycle(&mut self, timing: &Timing) {
        self.used.retain(|&(first, _)| first != timing.base);
    }
}

/// Read a completed submission's timestamps as pass timings and its whole span.
pub(super) fn resolve(
    device: &ProtocolObject<dyn MTLDevice>,
    timing: &Timing,
    queue: Queue,
) -> Result<(Vec<PassTiming>, TimestampSpan)> {
    let slots = timing.slots();
    // SAFETY: the range lies inside the heap. The completed submission wrote every
    // slot in it (begin/end of the submission and of each pass) except aliases,
    // whose values are replaced below.
    let data = unsafe {
        timing
            .heap
            .resolveCounterRange(NSRange::new(timing.base, slots))
    }
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
    let mut ticks: Vec<u64> = (0..slots)
        .map(|slot| {
            let at = slot * stride;
            let mut tick = [0; 8];
            tick.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(tick)
        })
        .collect();
    for &(slot, written) in &timing.aliases {
        ticks[slot] = ticks[written];
    }
    let ms = |slot: usize| ticks[slot] as f64 * scale;
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
