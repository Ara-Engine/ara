//! One submitted Scene frame and two retained color/ID pairs. UI reads only shown.
use super::*;
use ara_gpu::{
    Barrier,
    vulkan::{Command, Completion},
};
mod completion;
#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct Live {
    shown: Option<Output>,
    spare: Option<Output>,
    pending: Option<Pending>,
    /// A resize requested while a frame was pending: (render, display) sizes.
    pub resize: Option<([u32; 2], [u32; 2])>,
    completed_timings: Option<Vec<PassTiming>>,
}
struct Pending {
    completion: Completion,
    output: Output,
    publish: bool,
}
pub(super) struct Output {
    color: Image,
    ids: Image,
    size: [u32; 2],
    id_size: [u32; 2],
    labels: Vec<(u64, String)>,
    epoch: Option<u64>,
    written: bool,
}
impl Output {
    fn new(gpu: &mut VulkanDevice, size: [u32; 2], id_size: [u32; 2]) -> Result<Self> {
        let usage = vk::ImageUsageFlags::SAMPLED
            | vk::ImageUsageFlags::TRANSFER_SRC
            | vk::ImageUsageFlags::TRANSFER_DST;
        let color = gpu.create_image(size[0], size[1], vk::Format::R8G8B8A8_UNORM, usage)?;
        let ids = match gpu.create_image(id_size[0], id_size[1], vk::Format::R32_SFLOAT, usage) {
            Ok(ids) => ids,
            Err(error) => {
                gpu.retire(Resource::Image(color))?;
                return Err(error.into());
            }
        };
        Ok(Self {
            color,
            ids,
            size,
            id_size,
            labels: Vec::new(),
            epoch: None,
            written: false,
        })
    }
    fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.color))?;
        gpu.retire(Resource::Image(self.ids))?;
        Ok(())
    }
    pub(super) fn copy(&mut self, renderer: &mut Renderer) -> Result<()> {
        ensure!(
            [OUTPUT, ID]
                .iter()
                .all(|&i| renderer.targets.states[i].access == Access::CopySource
                    && renderer.targets.states[i].queue == Queue::Graphics),
            "Scene composition did not finish its color/ID targets"
        );
        let mut commands = renderer.gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("Scene.Publish".into()));
        for (source, destination) in [
            (renderer.targets.images[OUTPUT], self.color),
            (renderer.targets.images[ID], self.ids),
        ] {
            commands.barriers([Barrier {
                resource: Resource::Image(destination),
                before: if self.written {
                    Access::CopySource
                } else {
                    Access::Undefined
                },
                after: Access::CopyDestination,
                handoff: None,
            }]);
            commands.push(Command::CopyImage {
                source,
                destination,
            });
            commands.barriers([Barrier {
                resource: Resource::Image(destination),
                before: Access::CopyDestination,
                after: Access::CopySource,
                handoff: None,
            }]);
        }
        commands.push(Command::EndPass);
        renderer.gpu.submit(commands.finish(), &[])?;
        self.written = true;
        Ok(())
    }
}
impl Live {
    pub fn rename(&mut self, entity: u64, name: &str) {
        for output in [
            self.shown.as_mut(),
            self.spare.as_mut(),
            self.pending.as_mut().map(|p| &mut p.output),
        ]
        .into_iter()
        .flatten()
        {
            for (_, label) in output.labels.iter_mut().filter(|(id, _)| *id == entity) {
                label.clear();
                label.push_str(name);
            }
        }
    }
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn color(&self) -> Option<Image> {
        self.shown.as_ref().map(|o| o.color)
    }
    pub fn ids(&self) -> Option<Image> {
        self.shown.as_ref().map(|o| o.ids)
    }
    pub fn labels(&self) -> Option<&[(u64, String)]> {
        self.shown.as_ref().map(|o| o.labels.as_slice())
    }
    pub fn size(&self) -> Option<[u32; 2]> {
        self.shown.as_ref().map(|o| o.size)
    }
}

impl Renderer {
    /// Whether a Scene or secondary frame still owns shared frame data.
    pub fn scene_frame_pending(&self) -> bool {
        self.views.pending() || self.live.as_ref().is_some_and(Live::pending)
    }
    /// True once the interactive Scene uses retained completed color and IDs.
    pub fn scene_frame_async(&self) -> bool {
        self.live.is_some()
    }

    /// Poll the submitted Scene without a host wait. False keeps displaying the
    /// previous image; true permits applying coalesced input and submitting again.
    pub fn poll_scene_frame(&mut self) -> Result<bool> {
        if !self.views.poll(&mut self.gpu)? {
            return Ok(false);
        }
        let Some(live) = &mut self.live else {
            return Ok(true);
        };
        if !live.poll(&mut self.gpu)? {
            return Ok(false);
        }
        if let Some(timings) = live.completed_timings.take() {
            self.last_timings = timings;
        }
        let resize = live.resize.take();
        if let Some((render, display)) = resize {
            self.resize_upscaled(render, display)?;
        }
        Ok(true)
    }

    /// Capture and shutdown boundary. UI frames poll instead of calling this.
    pub fn wait_scene_frame(&mut self) -> Result<()> {
        // Explicit capture/shutdown boundaries do not defer to tab scheduling.
        self.views.cancel_request();
        self.views.wait(&mut self.gpu)?;
        if let Some(live) = &mut self.live {
            live.wait(&mut self.gpu)?;
        }
        self.poll_scene_frame()?;
        Ok(())
    }

    /// Submit one complete Scene composition, including effects and post, without
    /// waiting for GPU completion. False coalesces input while the prior frame is
    /// pending. The caller keeps the last completed image attached to its UI.
    pub fn submit_scene_frame(
        &mut self,
        render: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<bool> {
        if !self.poll_scene_frame()? || self.views.take_request() {
            return Ok(false);
        }
        let id_size = self.gpu.image_extent(self.targets.images[ID])?;
        let mut live = self.live.take().unwrap_or_default();
        let mut output = match live.acquire(&mut self.gpu, self.size, id_size) {
            Ok(output) => output,
            Err(error) => {
                self.live = Some(live);
                return Err(error);
            }
        };
        if let Err(error) = self.gpu.begin_deferred_completion() {
            live.spare = Some(output);
            self.live = Some(live);
            return Err(error.into());
        }
        let timings = std::mem::take(&mut self.last_timings);
        let result = render(self).and_then(|()| {
            let id_size = self.gpu.image_extent(self.targets.images[ID])?;
            if output.size != self.size || output.id_size != id_size {
                let next = Output::new(&mut self.gpu, self.size, id_size)?;
                std::mem::replace(&mut output, next).retire(&mut self.gpu)?;
            }
            // Snapshot the ID table after a publication, before its image is
            // exposed. Camera-only frames do not visit or copy entity labels.
            if output.epoch != Some(self.entity_epoch) {
                output.labels.clear();
                output.labels.extend(
                    self.content
                        .entity_names
                        .iter()
                        .map(|(id, name)| (*id, name.to_owned())),
                );
                output.epoch = Some(self.entity_epoch);
            }
            output.copy(self)
        });
        self.last_timings = timings;
        let completion = self.gpu.end_deferred_completion()?;
        live.pending = Some(Pending {
            completion,
            output,
            publish: result.is_ok(),
        });
        self.live = Some(live);
        result?;
        Ok(true)
    }

    /// Copy a synchronously completed capture frame into the retained UI pair.
    pub(super) fn publish_sync_scene(&mut self) -> Result<()> {
        if self.live.is_some() && !self.secondary_active && !self.gpu.deferring_completion() {
            let mut timings = std::mem::take(&mut self.last_timings);
            self.submit_scene_frame(|_| Ok(()))?;
            self.wait_scene_frame()?;
            timings.append(&mut self.last_timings);
            self.last_timings = timings;
        }
        Ok(())
    }

    pub(super) fn pick_completed_scene(&mut self, pixel: [u32; 2]) -> Result<Option<u64>> {
        let Some(output) = self.live.as_ref().and_then(|live| live.shown.as_ref()) else {
            return Ok(None);
        };
        let pixel = std::array::from_fn(|i| {
            ((u64::from(pixel[i]) * u64::from(output.id_size[i])) / u64::from(self.size[i])) as u32
        });
        // SAFETY: Only the spare pair can be written by the pending Scene. The
        // shown pair was completed/published to the UI queue and stays immutable.
        let bytes = unsafe { self.gpu.read_ui_pixel(output.ids, pixel)? };
        let id = f32::from_le_bytes(bytes) as usize;
        Ok(id
            .checked_sub(1)
            .and_then(|id| output.labels.get(id))
            .map(|(id, _)| *id))
    }
}
