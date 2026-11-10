//! Explicit blocking BLAS compaction for setup and asset preparation.
use super::*;
impl MetalDevice {
    /// Compact a completed BLAS to a fresh identity. Blocking setup operation;
    /// rebuild TLAS references before retiring the source. Refits remain supported.
    pub fn compact_acceleration_structure(
        &mut self,
        source: AccelerationStructure,
    ) -> Result<AccelerationStructure> {
        native::pool(|| {
            if self.deferred.is_some() {
                return Err(GpuError::Contract(
                    "Compaction must be outside a frame scope".into(),
                ));
            }
            let raw = &self.accelerations.get(source)?.raw;
            let Geometry::Triangles(range) = raw.geometry else {
                return Err(GpuError::Contract(
                    "Compaction currently accepts BLAS geometry".into(),
                ));
            };
            if !raw.built || !raw.options.allow_compaction {
                return Err(GpuError::Contract(
                    "Compaction requires a built compaction-enabled BLAS".into(),
                ));
            }
            let original = raw.structure.clone();
            let options = raw.options;
            let scratch_size = self.buffers.get(raw.scratch)?.raw.size;
            let readback = self.create_readback_buffer(8)?;
            let output = &self.buffers.get(readback)?.raw;
            let output = MTL4BufferRange {
                bufferAddress: output.buffer.gpuAddress(),
                length: 8,
            };
            let point = self.submit_serialized(
                |encoder| {
                    encoder.barrierAfterQueueStages_beforeStages_visibilityOptions(
                        MTLStages::All,
                        MTLStages::AccelerationStructure,
                        MTL4VisibilityOptions::Device,
                    );
                    // SAFETY: source retained; writable eight-byte resident range, read after completion.
                    unsafe {
                        encoder.writeCompactedAccelerationStructureSize_toBuffer(&original, output);
                    }
                },
                |_, _| {},
            )?;
            self.wait(point)?;
            let bytes: [u8; 8] = self
                .read(readback)?
                .try_into()
                .map_err(|_| GpuError::Contract("Compacted size readback".into()))?;
            Device::retire(self, Resource::Buffer(readback))?;
            let size = usize::try_from(u64::from_le_bytes(bytes))
                .map_err(|_| GpuError::Contract("Invalid compacted size".into()))?;
            if size == 0 || size > original.size() {
                return Err(GpuError::Contract("Invalid compacted BLAS size".into()));
            }
            let (structure, heap) = self.memory.acceleration(&self.device, size)?;
            self.resident_heap(&heap);
            let scratch =
                self.create_buffer(scratch_size, vk::BufferUsageFlags::STORAGE_BUFFER, false)?;
            let destination = self.accelerations.insert(Owned::new(NativeAcceleration {
                structure,
                heap: Some(heap),
                geometry: Geometry::Triangles(range),
                scratch,
                dependencies: Vec::new(),
                auxiliary: vec![scratch],
                built: true,
                built_count: 0,
                compacted: true,
                options,
                capacity: 0,
            }))?;
            let compact = self.accelerations.get(destination)?.raw.structure.clone();
            let point = self.submit_serialized(
                |encoder| {
                    encoder.barrierAfterQueueStages_beforeStages_visibilityOptions(
                        MTLStages::All,
                        MTLStages::AccelerationStructure,
                        MTL4VisibilityOptions::Device,
                    );
                    // SAFETY: distinct resident structures; destination has the GPU-queried compacted size.
                    unsafe {
                        encoder.copyAndCompactAccelerationStructure_toAccelerationStructure(
                            &original, &compact,
                        );
                    }
                },
                |_, _| {},
            )?;
            self.wait(point)?;
            Ok(destination)
        })
    }
    /// Native bytes reserved for an acceleration structure, excluding scratch.
    pub fn acceleration_size(&self, structure: AccelerationStructure) -> Result<u64> {
        Ok(self.accelerations.get(structure)?.raw.structure.size() as u64)
    }
}
