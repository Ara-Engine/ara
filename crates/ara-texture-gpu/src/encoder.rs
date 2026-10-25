//! Each worker holds a single device; its pipelines reference immutable cook keys.
use crate::dispatch::{Encoding, MAX_BLOCKS, Transfers, unavailable};
use crate::split::{Pipelines, SCRATCH_BYTES};
use ara_asset::{BlockEncoder, Cancellation, CookPreset, Digest, Error, Result};
use ara_asset_format::TextureFormat;
use ara_gpu::{Pipeline, cooked::CookedPipeline, vulkan::VulkanDevice};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

struct Kernel {
    manifest: CookedPipeline,
    directory: PathBuf,
    pipeline: Option<Pipeline>,
}
struct State {
    gpu: VulkanDevice,
    kernels: Vec<Kernel>,
    transfers: Option<Transfers>,
    timings: std::collections::BTreeMap<String, f64>,
}

/// One headless, raster-only Vulkan device and its immutable BC codec cohort.
pub struct Encoder {
    identity: String,
    state: Mutex<State>,
}
impl Encoder {
    /// Opens a cooked cohort and a driver-local pipeline cache, with no window or
    /// runtime shader compilation. Call only inside an isolated worker/background job.
    pub fn new(cooked: &Path, cache: &Path) -> Result<Self> {
        let mut identity = Vec::new();
        let mut kernels = Vec::new();
        for name in [
            "bc4",
            "bc5",
            "bc6h",
            "bc7",
            "bc7_mode0",
            "bc7_mode1",
            "bc7_mode2",
            "bc7_mode3",
            "bc7_mode4",
            "bc7_mode5",
            "bc7_mode6",
            "bc7_mode7",
            "bc7_select",
            "bc7_reduce",
        ] {
            let bytes = std::fs::read(cooked.join(format!("{name}.json"))).map_err(unavailable)?;
            let mut manifest: CookedPipeline =
                serde_json::from_slice(&bytes).map_err(unavailable)?;
            identity.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            identity.extend_from_slice(&bytes);
            if manifest.key.len() != 64 || !manifest.key.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(unavailable("codec cook key must be a content digest"));
            }
            if manifest.stages.len() != 1 || manifest.stages[0].stage != ara_gpu::cooked::Stage::Compute {
                return Err(unavailable("codec pipeline must contain one compute stage"));
            }
            let (stage_key, _) = manifest.stages[0].spirv.split_once('/').ok_or_else(|| unavailable("codec stage has no immutable artifact directory"))?;
            if stage_key.len() != 64 || !stage_key.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(unavailable("codec artifact directory must be a content digest"));
            }
            let prefix = format!("{stage_key}/");
            let directory = cooked.join(stage_key);
            for stage in &mut manifest.stages {
                stage.spirv = stage
                    .spirv
                    .strip_prefix(&prefix)
                    .ok_or_else(|| {
                        unavailable("codec stage must live under its immutable cook key")
                    })?
                    .to_owned();
                stage.dxil = stage
                    .dxil
                    .strip_prefix(&prefix)
                    .ok_or_else(|| {
                        unavailable("codec stage must live under its immutable cook key")
                    })?
                    .to_owned();
            }
            // This directory contains immutable stage artifacts, no mutable
            // pipeline manifests. Development hot reload cannot retarget an
            // in-flight encode while retaining its old content-addressed key.
            kernels.push(Kernel {
                manifest,
                directory,
                pipeline: None,
            });
        }
        let gpu = VulkanDevice::new_raster(&[], cache).map_err(unavailable)?;
        if !gpu.context().info.buffer_int64_atomics {
            return Err(unavailable(
                "integer BC6H/BC7 shaders require shaderInt64 on this device",
            ));
        }
        Ok(Self {
            identity: format!("gpu-bcn-cpu-normals-v1:{}", Digest::of(&identity)),
            state: Mutex::new(State {
                gpu,
                kernels,
                transfers: None,
                    timings: Default::default(),
            }),
        })
    }
}
impl BlockEncoder for Encoder {
    fn backend(&self) -> &str {
        &self.identity
    }
    fn metrics(&self) -> std::collections::BTreeMap<String, f64> {
        self.state.lock().map(|state| state.timings.clone()).unwrap_or_default()
    }
    fn encode(
        &self,
        format: TextureFormat,
        preset: CookPreset,
        blocks: &[u32],
        cancellation: &Cancellation,
    ) -> Result<Vec<u8>> {
        let index = match format {
            TextureFormat::Bc4 => 0,
            TextureFormat::Bc5 => 1,
            TextureFormat::Bc6h => 2,
            TextureFormat::Bc7 | TextureFormat::Bc7Srgb => 3,
            _ => return Err(Error::Invalid("GPU encoder requires a BC format".into())),
        };
        let words = if index == 2 { 32 } else { 16 };
        if !blocks.len().is_multiple_of(words) {
            return Err(Error::Invalid("truncated block-major texture input".into()));
        }
        cancellation.check()?;
        if blocks.is_empty() {
            return Ok(Vec::new());
        }
        let preset = match preset {
            CookPreset::Fast => 0,
            CookPreset::Balanced => 1,
            CookPreset::HighQuality => 2,
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| unavailable("GPU codec state poisoned"))?;
        let State {
            gpu,
            kernels,
            transfers,
            timings,
        } = &mut *state;
        let pipelines = if index == 3 && preset > 0 {
            let mut modes = [pipeline(gpu, &mut kernels[4])?; 8];
            for mode in 1..8 {
                modes[mode] = pipeline(gpu, &mut kernels[4 + mode])?;
            }
            Pipelines::Bc7 {
                modes,
                select: pipeline(gpu, &mut kernels[12])?,
                reduce: pipeline(gpu, &mut kernels[13])?,
            }
        } else {
            Pipelines::Single(pipeline(gpu, &mut kernels[index])?)
        };
        let max_count = (blocks.len() / words).min(MAX_BLOCKS);
        let input_capacity = max_count * words * 4;
        let output_capacity = max_count * format.block_bytes();
        let scratch_capacity = if pipelines.split() {
            max_count * SCRATCH_BYTES
        } else {
            0
        };
        if transfers
            .as_ref()
            .is_some_and(|t| !t.fits(input_capacity, output_capacity, scratch_capacity))
            && let Some(old) = transfers.take()
        {
            old.retire(gpu)?;
        }
        if transfers.is_none() {
            *transfers = Some(Transfers::new(
                gpu,
                input_capacity,
                output_capacity,
                scratch_capacity,
            )?);
        }
        let transfer = transfers.as_mut().expect("transfer set was allocated");
        let mut result = Vec::with_capacity(blocks.len() / words * format.block_bytes());
        for chunk in blocks.chunks(MAX_BLOCKS * words) {
            let (output, passes) = transfer.run(
                gpu,
                Encoding {
                    pipelines,
                    preset,
                    words: chunk,
                    count: chunk.len() / words,
                    block_bytes: format.block_bytes(),
                },
                cancellation,
            )?;
            for pass in passes { *timings.entry(format!("{}.gpu_ms", pass.pass)).or_default() += pass.milliseconds; }
            result.extend_from_slice(&output);
        }
        gpu.assert_clean().map_err(unavailable)?;
        Ok(result)
    }
}

fn pipeline(gpu: &mut VulkanDevice, kernel: &mut Kernel) -> Result<Pipeline> {
    if let Some(pipeline) = kernel.pipeline {
        return Ok(pipeline);
    }
    let pipeline = gpu
        .create_pipeline(&kernel.manifest, &kernel.directory)
        .map_err(unavailable)?;
    kernel.pipeline = Some(pipeline);
    Ok(pipeline)
}
