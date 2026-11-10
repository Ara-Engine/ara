//! Automatic Metal heaps suballocate private buffers, textures and AS storage.
//! Each resource retains its heap. Only completed retirement drops the resource,
//! so Metal cannot reuse its range while either queue may still reach it.
use super::*;
const BLOCK: usize = 8 * 1024 * 1024;
pub(super) type Heap = Retained<ProtocolObject<dyn MTLHeap>>;
#[derive(Default)]
pub(super) struct Pool {
    heaps: Vec<Heap>,
    weak: Vec<objc2::rc::Weak<ProtocolObject<dyn MTLHeap>>>,
}
// SAFETY: Metal heaps and resources may be retained and allocated on any thread.
// A pool's metadata has one mutable owner, transferred to its preparation worker.
unsafe impl Send for Pool {}
impl Pool {
    fn heap(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        size: MTLSizeAndAlign,
    ) -> Result<Heap> {
        if let Some(heap) = self
            .heaps
            .iter()
            .find(|h| h.maxAvailableSizeWithAlignment(size.align) >= size.size)
        {
            return Ok(heap.clone());
        }
        self.weak.retain(|h| h.load().is_some());
        if let Some(heap) = self
            .weak
            .iter()
            .filter_map(|h| h.load())
            .find(|h| h.maxAvailableSizeWithAlignment(size.align) >= size.size)
        {
            return Ok(heap);
        }
        let desc = MTLHeapDescriptor::new();
        desc.setStorageMode(MTLStorageMode::Private);
        desc.setHazardTrackingMode(MTLHazardTrackingMode::Untracked);
        // Medium/large images rarely pack into a fixed block (for example three
        // half-resolution targets leave almost a target's worth of unusable tail).
        // Match those heaps to the request; retain blocks for small shared ranges.
        let bytes = if size.size >= BLOCK / 8 {
            size.size
        } else {
            BLOCK
        }
        .checked_next_multiple_of(65536)
        .ok_or_else(|| GpuError::Contract("Heap size overflow".into()))?;
        desc.setSize(bytes);
        let heap = device
            .newHeapWithDescriptor(&desc)
            .ok_or_else(|| nil("private heap"))?;
        self.heaps.push(heap.clone());
        Ok(heap)
    }
    pub fn buffer(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        size: usize,
    ) -> Result<(Retained<ProtocolObject<dyn MTLBuffer>>, Heap)> {
        let options = MTLResourceOptions::StorageModePrivate
            | MTLResourceOptions::HazardTrackingModeUntracked;
        let heap = self.heap(
            device,
            device.heapBufferSizeAndAlignWithLength_options(size, options),
        )?;
        let buffer = heap
            .newBufferWithLength_options(size, options)
            .ok_or_else(|| nil("heap buffer"))?;
        Ok((buffer, heap))
    }
    pub fn image(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        desc: &MTLTextureDescriptor,
    ) -> Result<(Retained<ProtocolObject<dyn MTLTexture>>, Heap)> {
        desc.setHazardTrackingMode(MTLHazardTrackingMode::Untracked);
        let heap = self.heap(device, device.heapTextureSizeAndAlignWithDescriptor(desc))?;
        let texture = heap
            .newTextureWithDescriptor(desc)
            .ok_or_else(|| nil("heap texture"))?;
        Ok((texture, heap))
    }
    pub fn acceleration(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        size: usize,
    ) -> Result<(Retained<ProtocolObject<dyn MTLAccelerationStructure>>, Heap)> {
        // SAFETY: callers checked the RT tier and pass a nonzero queried structure size.
        let requirements = unsafe { device.heapAccelerationStructureSizeAndAlignWithSize(size) };
        let heap = self.heap(device, requirements)?;
        // SAFETY: this private heap has the native capacity/alignment for the structure.
        let structure = unsafe { heap.newAccelerationStructureWithSize(size) }
            .ok_or_else(|| nil("heap acceleration structure"))?;
        Ok((structure, heap))
    }
    /// A preparation worker leaves ownership with the returned resources/owner,
    /// retaining weak access for later jobs without pinning their peak allocation.
    pub fn release_ownership(&mut self) {
        self.weak
            .extend(self.heaps.drain(..).map(|h| objc2::rc::Weak::new(&*h)));
    }
    pub fn adopt(&mut self, heap: &Heap) {
        if !self.heaps.iter().any(|h| std::ptr::eq(&**h, &**heap)) {
            self.heaps.push(heap.clone());
        }
    }
    /// Keep at most one small empty heap; live resources include unpublished jobs.
    pub fn trim(&mut self, residency: &ProtocolObject<dyn MTLResidencySet>) -> bool {
        let mut spare = false;
        let mut changed = false;
        self.heaps.retain(|heap| {
            if heap.usedSize() != 0 {
                return true;
            }
            if !spare && heap.size() <= BLOCK {
                spare = true;
                return true;
            }
            residency.removeAllocation(ProtocolObject::from_ref(&**heap));
            changed = true;
            false
        });
        changed
    }
}
impl MetalDevice {
    pub(super) fn resident_heap(&mut self, heap: &Heap) {
        self.memory.adopt(heap);
        let allocation = ProtocolObject::from_ref(&**heap);
        if !self.residency.containsAllocation(allocation) {
            self.residency.addAllocation(allocation);
            self.residency_dirty = true;
        }
    }
}
