//! Explicit blocking BLAS compaction for setup and asset preparation.
use super::*;
impl VulkanDevice {
    /// Compact a built BLAS into a fresh identity. Blocking setup operation;
    /// rebuild TLAS references before retiring the source. Refits remain supported.
    pub fn compact_acceleration_structure(
        &mut self,
        source: AccelerationStructure,
    ) -> Result<AccelerationStructure> {
        if self.deferred.is_some() {
            return Err(GpuError::Contract(
                "Compaction must be outside a frame scope".into(),
            ));
        }
        let source = self.accelerations.get(source)?;
        let raw = &source.raw;
        if !matches!(raw.geometry, Geometry::Triangles { .. })
            || !raw.built
            || !raw.options.allow_compaction
        {
            return Err(GpuError::Contract(
                "Compaction requires a built compaction-enabled BLAS".into(),
            ));
        }
        let original = raw.handle;
        let geometry = raw.geometry;
        let options = raw.options;
        let original_size = raw.backing.size;
        let queue = source.owner;
        let scratch_size = self.buffers.get(raw.scratch)?.raw.size;
        self.wait_idle()?;
        let mut guard = super::guard::Objects::new(&self.context.device);
        // SAFETY: this private query pool is reset and used only by the blocking submissions below.
        let query = unsafe {
            self.context.device.create_query_pool(
                &vk::QueryPoolCreateInfo::default()
                    .query_type(vk::QueryType::ACCELERATION_STRUCTURE_COMPACTED_SIZE_KHR)
                    .query_count(1),
                None,
            )
        }
        .map_err(anyhow::Error::from)?;
        guard.push(super::guard::Object::Query(query));
        self.context.immediate_on(queue == Queue::Compute, |cmd| {
            // SAFETY: source completed on both timelines; the global dependency publishes build writes to the size query.
            unsafe {
                self.context.device.cmd_reset_query_pool(cmd, query, 0, 1);
                as_dependency(&self.context.device, cmd);
                self.context
                    .acceleration
                    .cmd_write_acceleration_structures_properties(
                        cmd,
                        &[original],
                        vk::QueryType::ACCELERATION_STRUCTURE_COMPACTED_SIZE_KHR,
                        query,
                        0,
                    );
            }
        })?;
        let mut size = [0u64];
        // SAFETY: the blocking submission completed; query zero wrote one 64-bit compacted size.
        unsafe {
            self.context.device.get_query_pool_results(
                query,
                0,
                &mut size,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT,
            )
        }
        .map_err(anyhow::Error::from)?;
        drop(guard);
        let size = size[0];
        if size == 0 || size > original_size {
            return Err(GpuError::Contract("Invalid compacted BLAS size".into()));
        }
        let scratch = self.create_buffer(
            scratch_size,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            false,
        )?;
        let (handle, backing) = self
            .context
            .structure(size, vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)?;
        self.context.immediate_on(queue == Queue::Compute, |cmd| {
            // SAFETY: distinct live structures; destination has the GPU-queried compacted size.
            unsafe {
                as_dependency(&self.context.device, cmd);
                self.context.acceleration.cmd_copy_acceleration_structure(
                    cmd,
                    &vk::CopyAccelerationStructureInfoKHR::default()
                        .src(original)
                        .dst(handle)
                        .mode(vk::CopyAccelerationStructureModeKHR::COMPACT),
                );
            }
        })?;
        // SAFETY: the newly compacted BLAS and backing remain alive in this context.
        let address = unsafe {
            self.context
                .acceleration
                .get_acceleration_structure_device_address(
                    &vk::AccelerationStructureDeviceAddressInfoKHR::default()
                        .acceleration_structure(handle),
                )
        };
        self.accelerations.insert(Owned {
            raw: NativeAcceleration {
                handle,
                address,
                backing,
                scratch,
                dependencies: Vec::new(),
                auxiliary: vec![scratch],
                built: true,
                built_count: 0,
                compacted: true,
                geometry,
                options,
                capacity: 0,
            },
            last: [0; 2],
            heap: None,
            owner: queue,
        })
    }
    /// Native bytes reserved for an acceleration structure, excluding scratch.
    pub fn acceleration_size(&self, structure: AccelerationStructure) -> Result<u64> {
        Ok(self.accelerations.get(structure)?.raw.backing.size)
    }
}
