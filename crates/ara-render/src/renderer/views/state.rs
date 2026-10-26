//! Camera-local resources; pipeline handles and uploaded scene content are shared.
use super::*;
use crate::look::gpu::{LookGpu, Pipelines as LookPipelines, STATE_BYTES};

pub(super) struct ViewState {
    size: [u32; 2],
    render: [u32; 2],
    targets: Targets,
    frame: Buffer,
    frame_index: u32,
    display_frame: Buffer,
    display_frame_index: u32,
    compute_frame: Buffer,
    compute_frame_index: u32,
    previous_vp: Mat4,
    frame_number: u32,
    last_timings: Vec<PassTiming>,
    pub look: LookGpu,
    native: Option<native::NativeView>,
    source_hash: String,
}

impl ViewState {
    pub fn new(renderer: &mut Renderer, size: [u32; 2]) -> Result<Self> {
        let mut targets = Some(Renderer::targets(&mut renderer.gpu, size, size)?);
        let mut allocated = Vec::new();
        let result = (|| {
            let (frame, frame_index) = buffer(
                &mut renderer.gpu,
                std::mem::size_of::<frame::Frame>(),
                &mut allocated,
            )?;
            let (display_frame, display_frame_index) = buffer(
                &mut renderer.gpu,
                std::mem::size_of::<frame::Frame>(),
                &mut allocated,
            )?;
            let (compute_frame, compute_frame_index) = buffer(
                &mut renderer.gpu,
                std::mem::size_of::<frame::Frame>(),
                &mut allocated,
            )?;
            let look = fork_look(&mut renderer.gpu, &renderer.look, size, &mut allocated)?;
            Ok(Self {
                size,
                render: size,
                targets: targets.take().context("secondary targets allocated")?,
                frame,
                frame_index,
                display_frame,
                display_frame_index,
                compute_frame,
                compute_frame_index,
                previous_vp: Mat4::IDENTITY,
                frame_number: 0,
                last_timings: Vec::new(),
                look,
                native: None,
                source_hash: renderer.content.base_hash.to_owned(),
            })
        })();
        if result.is_err() {
            if let Some(targets) = targets {
                retire_targets(&mut renderer.gpu, targets)?;
            }
            for resource in allocated {
                renderer.gpu.retire(resource)?;
            }
            renderer.gpu.collect()?;
        }
        result
    }

    pub fn prepare_native(&mut self, renderer: &mut Renderer) -> Result<()> {
        let mut reset = self.source_hash != renderer.content.base_hash;
        match &renderer.native {
            Some(source) => {
                if !self
                    .native
                    .as_ref()
                    .is_some_and(|view| view.matches(source))
                {
                    if let Some(old) = self.native.take() {
                        old.destroy(&mut renderer.gpu)?;
                    }
                    self.native = Some(source.new_view(&mut renderer.gpu)?);
                    reset = true;
                }
                if let Some(view) = &mut self.native {
                    reset |= view.refresh(source);
                }
            }
            None => {
                if let Some(old) = self.native.take() {
                    old.destroy(&mut renderer.gpu)?;
                    reset = true;
                }
            }
        }
        if reset {
            self.source_hash.clear();
            self.source_hash.push_str(&renderer.content.base_hash);
            self.targets.history = None;
            self.frame_number = 0;
        }
        Ok(())
    }

    pub fn swap(&mut self, renderer: &mut Renderer) {
        std::mem::swap(&mut self.size, &mut renderer.size);
        std::mem::swap(&mut self.render, &mut renderer.render);
        std::mem::swap(&mut self.targets, &mut renderer.targets);
        std::mem::swap(&mut self.frame, &mut renderer.frame);
        std::mem::swap(&mut self.frame_index, &mut renderer.frame_index);
        std::mem::swap(&mut self.display_frame, &mut renderer.display_frame);
        std::mem::swap(
            &mut self.display_frame_index,
            &mut renderer.display_frame_index,
        );
        std::mem::swap(&mut self.compute_frame, &mut renderer.compute_frame);
        std::mem::swap(
            &mut self.compute_frame_index,
            &mut renderer.compute_frame_index,
        );
        std::mem::swap(&mut self.previous_vp, &mut renderer.previous_vp);
        std::mem::swap(&mut self.frame_number, &mut renderer.frame_number);
        std::mem::swap(&mut self.last_timings, &mut renderer.last_timings);
        std::mem::swap(&mut self.look, &mut renderer.look);
        if let (Some(source), Some(view)) = (&mut renderer.native, &mut self.native) {
            source.swap_view(view);
        }
    }

    pub fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        retire_targets(gpu, self.targets)?;
        for buffer in [
            self.frame,
            self.display_frame,
            self.compute_frame,
            self.look.frame,
            self.look.state,
        ] {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        for image in self.look.images.into_iter().flatten() {
            gpu.retire(Resource::Image(image))?;
        }
        if let Some(far) = self.look.far {
            far.destroy(gpu)?;
        }
        if let Some(cascades) = self.look.cascades {
            cascades.destroy(gpu)?;
        }
        if let Some(local) = self.look.local {
            local.destroy(gpu)?;
        }
        if let Some(horizon) = self.look.horizon {
            horizon.destroy(gpu)?;
        }
        if let Some(vsm) = self.look.vsm {
            vsm.destroy(gpu)?;
        }
        if let Some(native) = self.native {
            native.destroy(gpu)?;
        }
        // No pipeline/scene/residency/GI resources belong to this camera.
        Ok(())
    }
}

fn retire_targets(gpu: &mut VulkanDevice, targets: Targets) -> Result<()> {
    for image in targets.images {
        gpu.retire(Resource::Image(image))?;
    }
    for buffer in [targets.tiles, targets.read_color, targets.read_ids] {
        gpu.retire(Resource::Buffer(buffer))?;
    }
    Ok(())
}

fn buffer(
    gpu: &mut VulkanDevice,
    bytes: usize,
    allocated: &mut Vec<Resource>,
) -> Result<(Buffer, u32)> {
    let buffer = gpu.create_buffer(bytes as u64, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    allocated.push(Resource::Buffer(buffer));
    Ok((buffer, gpu.storage_buffer(buffer)?.index))
}

fn fork_look(
    gpu: &mut VulkanDevice,
    source: &LookGpu,
    size: [u32; 2],
    allocated: &mut Vec<Resource>,
) -> Result<LookGpu> {
    let (frame, frame_index) = buffer(
        gpu,
        std::mem::size_of::<crate::look::frame::LookFrame>(),
        allocated,
    )?;
    let (state, state_index) = buffer(gpu, STATE_BYTES, allocated)?;
    gpu.upload(state, &[0; STATE_BYTES])?;
    let p = &source.pipelines;
    Ok(LookGpu {
        look: source.look,
        quality: crate::look::Quality::default(),
        pipelines: LookPipelines {
            transmittance: p.transmittance,
            multiscatter: p.multiscatter,
            sky_view: p.sky_view,
            sky_irradiance: p.sky_irradiance,
            sky_prefilter: p.sky_prefilter,
            fog: p.fog,
            exposure: p.exposure,
            bloom: p.bloom,
            post: p.post,
            gi_relight: p.gi_relight,
            gi_scatter: p.gi_scatter,
            ao_trace: p.ao_trace,
            ao_denoise: p.ao_denoise,
            gi_gather: p.gi_gather,
            ssr_pyramid: p.ssr_pyramid,
            ssr_trace: p.ssr_trace,
            horizon_sun: p.horizon_sun,
            horizon_sky: p.horizon_sky,
            vsm_mark: p.vsm_mark,
            vsm_project: p.vsm_project,
            gi_relight_vsm: p.gi_relight_vsm,
        },
        frame,
        frame_index,
        state,
        state_index,
        state_access: Access::HostWrite,
        images: source.images.map(|_| None),
        sampled: source.sampled.map(|_| 0),
        storage: source.storage.map(|_| 0),
        states: source.states.map(|_| State {
            access: Access::Undefined,
            queue: Queue::Graphics,
        }),
        render: size,
        display: size,
        sky: None,
        deferred: false,
        deferred_history: None,
        settle: 0,
        seeded: false,
        gi: None,
        far: None,
        cascades: None,
        local: None,
        horizon: None,
        vsm: None,
        probe_staging: Vec::new(),
        probe_writes: Vec::new(),
    })
}
