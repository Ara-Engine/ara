use super::*;
use ara_asset_format::TextureFormat;
use ara_gpu::{Barrier, Queue, commands::Command, vk};

struct Pending {
    index: usize,
    resident: Resident,
}

impl NativeResidency {
    pub(super) fn prepare(&mut self, gpu: &mut GpuDevice, initialize: bool) -> Result<NativeStats> {
        ensure!(
            self.initial.as_ref().is_none_or(initial::Initial::is_ready),
            "native texture allocation is still pending"
        );
        if !self.dirty && self.stats.pending_textures == 0 {
            self.stats.uploaded_bytes = 0;
            self.stats.upload_gpu_ms = 0.0;
            return Ok(self.stats);
        }
        if let Some(point) = self.last {
            ensure!(
                gpu.completed()?[point.queue.index()] >= point.value,
                "native texture frame in flight"
            );
        }
        let (_, requested) =
            plan::select(&self.entries, self.config.resident_bytes, &mut self.plan)?;
        let mut selected = Vec::new();
        let mut published = self.stats.resident_bytes;
        let mut uploaded = 0;
        // Release expensive tails before promoting others. A partial refinement
        // therefore never exceeds the published payload budget.
        for promotion in [false, true] {
            for (index, entry) in self.entries.iter().enumerate() {
                let target = self.plan[index];
                if entry.resident.is_some_and(|r| r.row.0[1] == target) {
                    continue;
                }
                let bytes = plan::bytes(entry, target)?;
                let old_bytes = entry.resident.map_or(0, |r| r.bytes);
                if (bytes > old_bytes) != promotion {
                    continue;
                }
                let next = published - old_bytes + bytes;
                if !initialize
                    && (next > self.config.resident_bytes
                        || (!selected.is_empty() && uploaded + bytes > self.config.upload_bytes))
                {
                    continue;
                }
                selected.push(index);
                published = next;
                uploaded += bytes;
            }
        }
        if selected.is_empty() && (self.table.is_some() || self.entries.is_empty()) {
            self.stats.requested_bytes = requested;
            self.stats.uploaded_bytes = 0;
            self.stats.upload_gpu_ms = 0.0;
            self.dirty = false;
            return Ok(self.stats);
        }
        self.rows.clear();
        self.rows.extend(
            self.entries
                .iter()
                .map(|e| e.resident.map_or(Row::default(), |r| r.row)),
        );
        // Every new allocation belongs to this transaction until the GPU confirms
        // publication. Errors leave the previous entries/table usable.
        let mut allocated = Vec::new();
        let prepared = self.upload(gpu, &selected, &mut allocated);
        let (pending, table, point, milliseconds) = match prepared {
            Ok(result) => result,
            Err(error) => {
                for resource in allocated {
                    gpu.retire(resource)?;
                }
                if let Some(initial) = self.initial.take() {
                    initial.destroy(gpu)?;
                }
                gpu.collect()?;
                return Err(error);
            }
        };
        for pending in pending {
            if let Some(old) = self.entries[pending.index]
                .resident
                .replace(pending.resident)
                && !old.borrowed
            {
                gpu.retire(Resource::Image(old.image))?;
            }
        }
        let published_table = table.buffer;
        if let Some(old) = self.table.replace(table) {
            gpu.retire(Resource::Buffer(old.buffer))?;
        }
        // Only staging buffers are discarded after a successful transaction.
        for resource in allocated {
            if matches!(resource, Resource::Buffer(buffer) if buffer != published_table) {
                gpu.retire(resource)?;
            }
        }
        if let Some(initial) = self.initial.take() {
            initial.destroy(gpu)?;
        }
        gpu.collect()?;
        self.last = Some(point);
        self.dirty = false;
        self.stats = NativeStats {
            textures: self.entries.len(),
            resident_bytes: published,
            requested_bytes: requested,
            pending_textures: self
                .entries
                .iter()
                .zip(&self.plan)
                .filter(|(entry, mip)| entry.resident.is_none_or(|r| r.row.0[1] != **mip))
                .count(),
            uploaded_bytes: uploaded,
            upload_gpu_ms: milliseconds,
        };
        Ok(self.stats)
    }

    fn upload(
        &mut self,
        gpu: &mut GpuDevice,
        selected: &[usize],
        allocated: &mut Vec<Resource>,
    ) -> Result<(Vec<Pending>, Table, TimelinePoint, f64)> {
        let mut commands = gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("Texture.Native.Upload".into()));
        let mut pending = Vec::with_capacity(selected.len());
        for &index in selected {
            let source = &self.entries[index].source;
            let first = self.plan[index];
            let mip = source
                .mip(first)
                .ok_or_else(|| anyhow::anyhow!("native mip plan"))?;
            let (format, encoding) = format(&self.entries[index])?;
            let bytes = plan::bytes(&self.entries[index], first)?;
            let (image, stage) = if let Some(initial) = &mut self.initial {
                let (image, stage) = initial
                    .take_tail(index)
                    .ok_or_else(|| anyhow::anyhow!("native prepared mip tail is missing"))?;
                allocated.extend([Resource::Image(image), Resource::Buffer(stage)]);
                (image, stage)
            } else {
                let image = gpu.create_mip_image(
                    mip.size,
                    format,
                    source.mip_count() - first,
                    vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::TRANSFER_DST
                        | vk::ImageUsageFlags::TRANSFER_SRC,
                )?;
                allocated.push(Resource::Image(image));
                let stage = gpu.create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
                allocated.push(Resource::Buffer(stage));
                gpu.upload(stage, &source.stage(first)?)?;
                (image, stage)
            };
            let slot = gpu.sampled_image(image, false)?.index;
            let [width, height] = source.size();
            let row = Row([
                slot,
                first,
                width,
                height,
                encoding,
                source.mip_count() - first,
                0,
                0,
            ]);
            self.rows[index] = row;
            commands.barriers([
                barrier(
                    Resource::Buffer(stage),
                    Access::HostWrite,
                    Access::CopySource,
                ),
                barrier(
                    Resource::Image(image),
                    Access::Undefined,
                    Access::CopyDestination,
                ),
            ]);
            commands.push(Command::UploadImage {
                source: stage,
                destination: image,
            });
            commands.barriers([barrier(
                Resource::Image(image),
                Access::CopyDestination,
                Access::ShaderRead,
            )]);
            pending.push(Pending {
                index,
                resident: Resident {
                    image,
                    row,
                    bytes,
                    borrowed: false,
                },
            });
        }
        let buffer = match &mut self.initial {
            Some(initial) => initial
                .take_table()
                .ok_or_else(|| anyhow::anyhow!("native prepared table is missing"))?,
            None => gpu.create_buffer(
                (self.rows.len() * std::mem::size_of::<Row>()) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )?,
        };
        allocated.push(Resource::Buffer(buffer));
        // A prefetched table may have been conservatively retained by an earlier
        // geometry/last-good submission. Fill it through the ordered scope then.
        gpu.upload_queued(buffer, bytemuck::cast_slice(&self.rows))?;
        let binding = NativeBinding {
            buffer_index: gpu.storage_buffer(buffer)?.index,
            textures: u32::try_from(self.rows.len())?,
        };
        commands.barriers([barrier(
            Resource::Buffer(buffer),
            Access::HostWrite,
            Access::ShaderRead,
        )]);
        commands.push(Command::EndPass);
        let point = gpu.submit(commands.finish(), &[])?;
        let milliseconds = gpu.resolve(point)?.iter().map(|t| t.milliseconds).sum();
        Ok((pending, Table { buffer, binding }, point, milliseconds))
    }
}

pub(super) fn format(entry: &Entry) -> Result<(vk::Format, u32)> {
    Ok(match entry.source.format() {
        TextureFormat::Bc4 => (vk::Format::BC4_UNORM_BLOCK, 1),
        TextureFormat::Bc5 => (vk::Format::BC5_UNORM_BLOCK, 2),
        TextureFormat::Bc7 => (vk::Format::BC7_UNORM_BLOCK, 0),
        TextureFormat::Bc7Srgb => (
            if entry.srgb {
                vk::Format::BC7_SRGB_BLOCK
            } else {
                vk::Format::BC7_UNORM_BLOCK
            },
            0,
        ),
        _ => anyhow::bail!("unsupported native texture encoding"),
    })
}

fn barrier(resource: Resource, before: Access, after: Access) -> Barrier {
    Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}
