//! Captures of the Scene read back from Renderlab: the completed colour image and the packed id
//! image beside it (`ids`), assembled with `Capture::from_render`, so numbered marks, the
//! legend and the entity lookup are the ones agents already read. Only the Lit view exists (the other view modes are not available yet).
use super::*;
use crate::gpu::IdHit;
use ara_frame::capture::{Capture, CaptureContext, ShaderMetadata};
use ara_gpu::{Access, Barrier, Device, Image, Queue, Resource, commands::Command, vk};

/// Reads a completed `CopySource` RGBA8 image of `size` as tightly packed rows.
fn read_rgba8(
    gpu: &mut ara_gpu::GpuDevice,
    image: Image,
    size: [u32; 2],
) -> anyhow::Result<Vec<u8>> {
    let read = gpu.create_buffer(
        u64::from(size[0]) * u64::from(size[1]) * 4,
        vk::BufferUsageFlags::TRANSFER_DST,
        true,
    )?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.push(Command::ReadImage {
        source: image,
        destination: read,
    });
    commands.barriers([Barrier {
        resource: Resource::Buffer(read),
        before: Access::CopyDestination,
        after: Access::HostRead,
        handoff: None,
    }]);
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    let result = gpu.read(read);
    gpu.retire(Resource::Buffer(read))?;
    gpu.collect()?;
    Ok(result?)
}

impl SceneView {
    /// The Scene as Renderlab last drew it, as a capture of `request`'s size: the display image,
    /// the numbered one when marks are asked for, the legend, and each pixel's entity (from the
    /// ids the engine wrote beside the image). The Scene must have drawn a frame (`render_capture`).
    pub(crate) fn capture(
        &self,
        device: &mut SceneDevice,
        request: &CaptureRequest,
    ) -> anyhow::Result<Capture> {
        anyhow::ensure!(
            matches!(request.mode, ViewMode::Lit),
            "the {:?} view is not available yet: Renderlab draws the Lit view only",
            request.mode
        );
        let scene = self
            .renderlab
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("the Scene has not drawn a Renderlab frame yet"))?;
        let image = scene
            .image()
            .ok_or_else(|| anyhow::anyhow!("the Scene has not drawn a Renderlab frame yet"))?;
        anyhow::ensure!(
            device.size() == request.size,
            "the capture is {:?} but the Scene view is {:?}",
            request.size,
            device.size()
        );
        let host = scene
            .id_host()
            .ok_or_else(|| anyhow::anyhow!("the Scene has no object ids yet"))?;
        let names = host.entities.to_vec();
        // The ids per pixel: the texels the engine delivered by CPU copy, or its id image read
        // back (the shared ring and Apple's device hand across an image).
        let packed = match host.texels() {
            Some(texels) => texels.to_vec(),
            None => {
                let ids = scene
                    .id_image()
                    .ok_or_else(|| anyhow::anyhow!("the Scene has no object ids yet"))?;
                read_rgba8(device.gpu_mut(), ids, request.size)?
            }
        };
        let ids: Vec<u32> = packed
            .as_chunks::<4>()
            .0
            .iter()
            .map(|t| IdHit::from_packed(*t).id().unwrap_or(0))
            .collect();
        let rgba = read_rgba8(device.gpu_mut(), image, request.size)?;
        let hash = format!("{:?}", self.revision);
        let shaders = ShaderMetadata {
            content_key: "renderlab".into(),
            compiler: String::new(),
            binding_layout_hash: String::new(),
        };
        Capture::from_render(
            rgba,
            ids,
            request,
            CaptureContext {
                names: &names,
                hash: &hash,
                device: device.gpu().info(),
                timings: &[],
                shaders: &shaders,
            },
        )
    }

    /// Draws the Scene through Renderlab and returns once its content has been extracted,
    /// adopted and drawn for the settle passes: the blocking boundary a capture reads at.
    pub(in crate::studio) fn render_capture(
        &mut self,
        device: &mut SceneDevice,
        editor: &Editor,
        view: &crate::world::DocumentView,
        _settle: u32,
    ) -> ara_ui::Result<()> {
        loop {
            self.render_scene_frame(device, editor, view)?;
            if let Some(error) = &self.renderlab_error {
                return Err(format!("Renderlab could not draw the Scene: {error}").into());
            }
            let drawn = self.frame.is_some()
                && self.content_job.is_none()
                && self.content_ready.is_none()
                && self.scene_settle == 0
                && self
                    .renderlab
                    .as_ref()
                    .is_some_and(|scene| scene.image().is_some());
            if drawn {
                return Ok(());
            }
            if self.content_job.is_some() {
                // The worker is extracting: wait for it rather than spin.
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}
