//! Reuse one bounded transfer set and record separate upload/encode/readback passes.
use crate::split::Pipelines;
use ara_asset::{Cancellation, Error, Result};
use ara_gpu::{
    Access, Barrier, Buffer, Device, Queue, Resource, vk,
    vulkan::{Command, VulkanDevice},
};

pub(super) const MAX_BLOCKS: usize = 65_535;

pub(super) struct Encoding<'a> {
    pub(super) pipelines: Pipelines<'a>,
    pub(super) preset: u32,
    pub(super) words: &'a [u32],
    pub(super) count: usize,
    pub(super) block_bytes: usize,
}

pub(super) fn unavailable(error: impl std::fmt::Display) -> Error {
    Error::EncoderUnavailable(error.to_string())
}
fn edge(buffer: Buffer, before: Access, after: Access) -> Barrier {
    Barrier {
        resource: Resource::Buffer(buffer),
        before,
        after,
        handoff: None,
    }
}

pub(super) struct Transfers {
    staging: Buffer,
    input: Buffer,
    output: Buffer,
    readback: Buffer,
    input_slot: u32,
    output_slot: u32,
    input_capacity: usize,
    output_capacity: usize,
    scratch: Option<(Buffer, u32)>,
    scratch_capacity: usize,
    used: bool,
}
impl Transfers {
    pub(super) fn new(
        gpu: &mut VulkanDevice,
        input_capacity: usize,
        output_capacity: usize,
        scratch_capacity: usize,
    ) -> Result<Self> {
        let staging = gpu
            .create_buffer(
                input_capacity as u64,
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
            )
            .map_err(unavailable)?;
        let input = gpu
            .create_buffer(
                input_capacity as u64,
                vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::STORAGE_BUFFER,
                false,
            )
            .map_err(unavailable)?;
        let output = gpu
            .create_buffer(
                output_capacity as u64,
                vk::BufferUsageFlags::TRANSFER_SRC | vk::BufferUsageFlags::STORAGE_BUFFER,
                false,
            )
            .map_err(unavailable)?;
        let readback = gpu
            .create_readback_buffer(output_capacity as u64)
            .map_err(unavailable)?;
        let input_slot = gpu.storage_buffer(input).map_err(unavailable)?.index;
        let output_slot = gpu.storage_buffer(output).map_err(unavailable)?.index;
        let scratch = if scratch_capacity > 0 {
            let buffer = gpu
                .create_buffer(
                    scratch_capacity as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    false,
                )
                .map_err(unavailable)?;
            Some((
                buffer,
                gpu.storage_buffer(buffer).map_err(unavailable)?.index,
            ))
        } else {
            None
        };
        Ok(Self {
            staging,
            input,
            output,
            readback,
            input_slot,
            output_slot,
            input_capacity,
            output_capacity,
            scratch,
            scratch_capacity,
            used: false,
        })
    }
    pub(super) fn fits(&self, input: usize, output: usize, scratch: usize) -> bool {
        input <= self.input_capacity
            && output <= self.output_capacity
            && scratch <= self.scratch_capacity
    }
    pub(super) fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        if let Some((buffer, _)) = self.scratch {
            gpu.retire(Resource::Buffer(buffer)).map_err(unavailable)?;
        }
        for buffer in [self.staging, self.input, self.output, self.readback] {
            gpu.retire(Resource::Buffer(buffer)).map_err(unavailable)?;
        }
        gpu.collect().map_err(unavailable)?;
        Ok(())
    }
    pub(super) fn run(
        &mut self,
        gpu: &mut VulkanDevice,
        encoding: Encoding<'_>,
        cancel: &Cancellation,
    ) -> Result<(Vec<u8>, std::collections::BTreeMap<String, f64>)> {
        cancel.check()?;
        let input: &[u8] = bytemuck::cast_slice(encoding.words);
        let bytes = encoding.count * encoding.block_bytes;
        let start = std::time::Instant::now();
        gpu.upload(self.staging, input).map_err(unavailable)?;
        let upload_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut commands = gpu.begin(Queue::Compute);
        commands.push(Command::BeginPass("Texture.Upload".into()));
        commands.barriers([
            edge(self.staging, Access::HostWrite, Access::CopySource),
            edge(
                self.input,
                if self.used {
                    Access::ShaderRead
                } else {
                    Access::Undefined
                },
                Access::CopyDestination,
            ),
        ]);
        commands.push(Command::CopyBuffer {
            source: self.staging,
            destination: self.input,
            size: input.len() as u64,
        });
        commands.barriers([
            edge(self.input, Access::CopyDestination, Access::ShaderRead),
            edge(self.staging, Access::CopySource, Access::HostWrite),
            edge(
                self.output,
                if self.used {
                    Access::CopySource
                } else {
                    Access::Undefined
                },
                Access::Storage,
            ),
        ]);
        commands.push(Command::EndPass);
        if encoding.pipelines.split() {
            let (scratch, _) = self.scratch.expect("split scratch was allocated");
            commands.barriers([edge(
                scratch,
                if self.used {
                    Access::Storage
                } else {
                    Access::Undefined
                },
                Access::Storage,
            )]);
        }
        commands.push(Command::BeginPass("Texture.Encode".into()));
        encoding.pipelines.record(
            &mut commands,
            [
                self.input_slot,
                self.output_slot,
                encoding.count as u32,
                encoding.preset,
                0,
                0,
                0,
                0,
            ],
            self.scratch,
        );
        commands.push(Command::EndPass);
        commands.push(Command::BeginPass("Texture.Readback".into()));
        commands.barriers([
            edge(self.output, Access::Storage, Access::CopySource),
            edge(
                self.readback,
                if self.used {
                    Access::HostRead
                } else {
                    Access::Undefined
                },
                Access::CopyDestination,
            ),
        ]);
        commands.push(Command::CopyBuffer {
            source: self.output,
            destination: self.readback,
            size: bytes as u64,
        });
        commands.barriers([edge(
            self.readback,
            Access::CopyDestination,
            Access::HostRead,
        )]);
        commands.push(Command::EndPass);
        let start = std::time::Instant::now();
        let submitted = gpu.submit(commands.finish(), &[]).map_err(unavailable)?;
        let submit_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = std::time::Instant::now();
        let passes = gpu.resolve(submitted).map_err(unavailable)?;
        let wait_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.used = true;
        cancel.check()?;
        let start = std::time::Instant::now();
        let mut readback = gpu.read(self.readback).map_err(unavailable)?;
        readback.truncate(bytes);
        let mut timings = std::collections::BTreeMap::from([
            ("upload.cpu_ms".into(), upload_ms),
            ("submit.cpu_ms".into(), submit_ms),
            ("wait.wall_ms".into(), wait_ms),
            (
                "readback.cpu_ms".into(),
                start.elapsed().as_secs_f64() * 1000.0,
            ),
            ("uploaded_bytes".into(), input.len() as f64),
            ("readback_bytes".into(), bytes as f64),
        ]);
        for pass in passes {
            *timings.entry(format!("{}.gpu_ms", pass.pass)).or_default() += pass.milliseconds;
        }
        Ok((readback, timings))
    }
}
