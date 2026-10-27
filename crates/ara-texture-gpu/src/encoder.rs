//! Each worker holds a single device; its pipelines reference immutable cook keys.
use crate::dispatch::{Encoding, MAX_BLOCKS, Transfers, unavailable};
use crate::split::Pipelines;
use ara_asset::{BlockEncoder, Cancellation, CookPreset, Digest, Error, Result};
use ara_asset_format::TextureFormat;
use ara_gpu::{GpuDevice, Pipeline, cooked::CookedPipeline};
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
    gpu: Option<GpuDevice>,
    kernels: Vec<Kernel>,
    transfers: Option<Transfers>,
    timings: std::collections::BTreeMap<String, f64>,
    dot: bool,
}

/// One headless, raster-only Vulkan device and its immutable BC codec cohort.
pub struct Encoder {
    identity: String,
    cache: PathBuf,
    allow_dot: bool,
    state: Mutex<State>,
}
impl Encoder {
    /// Opens immutable codec manifests. The headless device is created on first
    /// encode; texture-free model imports never initialize Vulkan.
    pub fn new(cooked: &Path, cache: &Path) -> Result<Self> {
        Self::with_optional_dot(cooked, cache, true)
    }

    // Tests also exercise the scalar artifacts on dot-capable hardware.
    pub(crate) fn with_optional_dot(cooked: &Path, cache: &Path, allow_dot: bool) -> Result<Self> {
        let start = std::time::Instant::now();
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
            "bc7_mode0_dot",
            "bc7_mode4_dot",
            "bc7_mode6_dot",
            "bc7_mode7_dot",
            "bc6h_rank",
            "bc6h_mode0",
            "bc6h_mode1",
            "bc6h_mode2",
            "bc6h_mode3",
            "bc6h_mode4",
            "bc6h_mode5",
            "bc6h_mode6",
            "bc6h_mode7",
            "bc6h_mode8",
            "bc6h_mode9",
            "bc6h_mode10",
            "bc6h_mode11",
            "bc6h_mode12",
            "bc6h_mode13",
            "bc6h_balanced0",
            "bc6h_balanced1",
            "bc6h_balanced5",
            "bc6h_balanced9",
            "bc6h_reduce",
        ] {
            let bytes = std::fs::read(cooked.join(format!("{name}.json"))).map_err(unavailable)?;
            let mut manifest: CookedPipeline =
                serde_json::from_slice(&bytes).map_err(unavailable)?;
            identity.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            identity.extend_from_slice(&bytes);
            if manifest.key.len() != 64 || !manifest.key.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(unavailable("codec cook key must be a content digest"));
            }
            if manifest.stages.len() != 1
                || manifest.stages[0].stage != ara_gpu::cooked::Stage::Compute
            {
                return Err(unavailable("codec pipeline must contain one compute stage"));
            }
            let (stage_key, _) = manifest.stages[0]
                .spirv
                .split_once('/')
                .ok_or_else(|| unavailable("codec stage has no immutable artifact directory"))?;
            if stage_key.len() != 64 || !stage_key.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(unavailable(
                    "codec artifact directory must be a content digest",
                ));
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
        Ok(Self {
            identity: format!("gpu-bcn-cpu-normals-v1:{}", Digest::of(&identity)),
            cache: cache.to_owned(),
            allow_dot,
            state: Mutex::new(State {
                gpu: None,
                kernels,
                transfers: None,
                dot: false,
                timings: std::collections::BTreeMap::from([(
                    "manifests.wall_ms".into(),
                    start.elapsed().as_secs_f64() * 1000.0,
                )]),
            }),
        })
    }
}
impl BlockEncoder for Encoder {
    fn backend(&self) -> &str {
        &self.identity
    }
    fn prepare(&self) -> Result<()> {
        // Warm the shared driver cache before disposable asset workers start.
        // This also proves the selected feature path actually creates and runs.
        self.encode(
            TextureFormat::Bc7,
            CookPreset::Balanced,
            &[0; 16],
            &Cancellation::default(),
        )?;
        Ok(())
    }
    fn metrics(&self) -> std::collections::BTreeMap<String, f64> {
        self.state
            .lock()
            .map(|state| state.timings.clone())
            .unwrap_or_default()
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
        if index == 2
            && blocks.as_chunks::<2>().0.iter().any(|&[rg, b]| {
                (rg & 0xffff) > 0x7bff || (rg >> 16) > 0x7bff || (b & 0xffff) > 0x7bff
            })
        {
            return Err(Error::Invalid(
                "BC6H input must contain finite nonnegative half codes".into(),
            ));
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
        if state.gpu.is_none() {
            let start = std::time::Instant::now();
            let gpu = GpuDevice::new_raster(&[], &self.cache).map_err(unavailable)?;
            if !gpu.info().buffer_int64_atomics {
                return Err(unavailable(
                    "integer BC6H/BC7 shaders require shaderInt64 on this device",
                ));
            }
            state.dot = self.allow_dot && gpu.info().packed_integer_dot_accelerated;
            state.gpu = Some(gpu);
            state.timings.insert(
                "initialize.wall_ms".into(),
                start.elapsed().as_secs_f64() * 1000.0,
            );
        }
        let State {
            gpu,
            kernels,
            transfers,
            timings,
            dot,
        } = &mut *state;
        let gpu = gpu.as_mut().expect("encoding initialized the device");
        let start = std::time::Instant::now();
        let mut modes;
        let mut hdr_modes;
        let mut hdr_balanced;
        let pipelines = if index == 3 && preset > 0 {
            let selected = if *dot {
                [14, 5, 6, 7, 15, 9, 16, 17]
            } else {
                [4, 5, 6, 7, 8, 9, 10, 11]
            };
            modes = [pipeline(gpu, &mut kernels[selected[0]])?; 8];
            for mode in 1..8 {
                modes[mode] = pipeline(gpu, &mut kernels[selected[mode]])?;
            }
            Pipelines::Bc7 {
                modes: &modes,
                select: pipeline(gpu, &mut kernels[12])?,
                reduce: pipeline(gpu, &mut kernels[13])?,
            }
        } else if index == 2 {
            hdr_modes = [pipeline(gpu, &mut kernels[29])?; 14];
            let range = match preset {
                0 => 10..11,
                1 => 10..14,
                _ => 0..14,
            };
            for mode in range {
                hdr_modes[mode] = pipeline(gpu, &mut kernels[19 + mode])?;
            }
            let balanced = if preset == 1 {
                hdr_balanced = [pipeline(gpu, &mut kernels[33])?; 4];
                for mode in 1..4 {
                    hdr_balanced[mode] = pipeline(gpu, &mut kernels[33 + mode])?;
                }
                Some((&hdr_balanced, pipeline(gpu, &mut kernels[18])?))
            } else {
                None
            };
            Pipelines::Bc6h(crate::bc6_split::Bc6Pipelines {
                modes: &hdr_modes,
                balanced,
                reduce: pipeline(gpu, &mut kernels[37])?,
            })
        } else {
            Pipelines::Single(pipeline(gpu, &mut kernels[index])?)
        };
        *timings.entry("pipelines.wall_ms".into()).or_default() +=
            start.elapsed().as_secs_f64() * 1000.0;
        let max_count = (blocks.len() / words).min(MAX_BLOCKS);
        let input_capacity = max_count * words * 4;
        let output_capacity = max_count * format.block_bytes();
        let scratch_capacity = max_count * pipelines.scratch_bytes();
        let start = std::time::Instant::now();
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
        *timings.entry("allocation.wall_ms".into()).or_default() +=
            start.elapsed().as_secs_f64() * 1000.0;
        let transfer = transfers.as_mut().expect("transfer set was allocated");
        let mut result = Vec::with_capacity(blocks.len() / words * format.block_bytes());
        for chunk in blocks.chunks(MAX_BLOCKS * words) {
            let (output, measured) = transfer.run(
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
            for (name, value) in measured {
                *timings.entry(name).or_default() += value;
            }
            result.extend_from_slice(&output);
        }
        gpu.assert_clean().map_err(unavailable)?;
        Ok(result)
    }
}

fn pipeline(gpu: &mut GpuDevice, kernel: &mut Kernel) -> Result<Pipeline> {
    if let Some(pipeline) = kernel.pipeline {
        return Ok(pipeline);
    }
    let pipeline = gpu
        .create_pipeline(&kernel.manifest, &kernel.directory)
        .map_err(unavailable)?;
    kernel.pipeline = Some(pipeline);
    Ok(pipeline)
}
