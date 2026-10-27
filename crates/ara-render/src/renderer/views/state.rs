//! Camera-local resources; pipeline handles and uploaded scene content are shared.
use super::*;
use crate::look::gpu::{LookGpu, STATE_BYTES};

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
    pub fn new(renderer: &mut Renderer, render: [u32; 2], size: [u32; 2]) -> Result<Self> {
        let resources =
            targets::allocate(&mut renderer.gpu, preparation::request(render, size, false))?;
        Self::from_prepared(renderer, render, size, resources).map(|(state, _)| state)
    }

    pub(super) fn matches_extent(&self, render: [u32; 2], display: [u32; 2]) -> bool {
        self.render == render && self.size == display
    }

    pub(super) fn from_prepared(
        renderer: &mut Renderer,
        render: [u32; 2],
        size: [u32; 2],
        mut resources: ara_gpu::vulkan::PreparedResources,
    ) -> Result<(Self, Vec<Image>)> {
        let owned = ara_gpu::vulkan::PreparedResources {
            images: resources.images.to_vec(),
            buffers: resources.buffers.to_vec(),
        };
        let result = (|| {
            ensure!(
                resources.buffers.len() == 8 && resources.images.len() >= TARGET_COUNT,
                "Invalid prepared camera"
            );
            let mut buffers = resources.buffers.split_off(3).into_iter();
            let output_images = resources.images.split_off(TARGET_COUNT);
            let targets = targets::bind(&mut renderer.gpu, resources)?;
            let mut next = || buffers.next().context("prepared camera constants");
            let frame = next()?;
            let display_frame = next()?;
            let compute_frame = next()?;
            let look_frame = next()?;
            let look_state = next()?;
            let look = fork_look(
                &mut renderer.gpu,
                &renderer.look,
                render,
                size,
                look_frame,
                look_state,
            )?;
            Ok((
                Self {
                    size,
                    render,
                    targets,
                    frame_index: renderer.gpu.storage_buffer(frame)?.index,
                    frame,
                    display_frame_index: renderer.gpu.storage_buffer(display_frame)?.index,
                    display_frame,
                    compute_frame_index: renderer.gpu.storage_buffer(compute_frame)?.index,
                    compute_frame,
                    previous_vp: Mat4::IDENTITY,
                    frame_number: 0,
                    last_timings: Vec::new(),
                    look,
                    native: None,
                    source_hash: renderer.content.base_hash.to_owned(),
                },
                output_images,
            ))
        })();
        if result.is_err() {
            targets::retire(&mut renderer.gpu, owned)?;
        }
        result
    }

    /// Target replacement retains the camera's quality maps and exposure state.
    /// Look::resize drops only view-sized images, leaving world/cascade caches.
    pub(super) fn inherit(
        &mut self,
        old: &mut Self,
        gpu: &mut VulkanDevice,
        render: [u32; 2],
        display: [u32; 2],
    ) -> Result<()> {
        // Resize the still-owned old look before swapping, so an allocation
        // failure cannot strand the newly prepared target batch.
        old.look.resize(gpu, render, display)?;
        std::mem::swap(&mut self.look, &mut old.look);
        self.native = old.native.take();
        Ok(())
    }
    pub fn prepare_native(&mut self, renderer: &mut Renderer) -> Result<()> {
        ara_gpu::cpu_scope!("Raster.secondary_native");
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

fn fork_look(
    gpu: &mut VulkanDevice,
    source: &LookGpu,
    render: [u32; 2],
    size: [u32; 2],
    frame: Buffer,
    state: Buffer,
) -> Result<LookGpu> {
    let frame_index = gpu.storage_buffer(frame)?.index;
    let state_index = gpu.storage_buffer(state)?.index;
    gpu.upload(state, &[0; STATE_BYTES])?;
    Ok(LookGpu {
        look: source.look,
        quality: crate::look::Quality::default(),
        pipelines: source.pipelines,
        frame,
        frame_index,
        state,
        state_index,
        state_access: Access::HostWrite,
        lut: source.lut,
        lut_index: source.lut_index,
        lut_size: source.lut_size,
        images: source.images.map(|_| None),
        sampled: source.sampled.map(|_| 0),
        storage: source.storage.map(|_| 0),
        states: source.states.map(|_| State {
            access: Access::Undefined,
            queue: Queue::Graphics,
        }),
        render,
        display: size,
        sky: None,
        deferred: false,
        frame_dof: false,
        frame_motion: false,
        frame_wire: false,
        unseen: false,
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
