//! The Scene's object ids as the editor reads them: the channels and, when the engine hands
//! them across as CPU texels, the texels.
//!
//! Renderlab writes a packed id image beside each frame (`ExternalFrame::id`, `id_texels`): one
//! RGBA8 texel per output pixel holding the little-endian id of the nearest opaque object, 0 for
//! nothing and `0xFFFF_FFFF` for a surface without an id. The id of draw `n` of the frame is
//! `n + 1`, then each resident terrain tile follows the draws ([`entities`]), so the editor's
//! channel map, outline groups and click path read them as they read any channel. The outlines,
//! hover and grid sample the image; a click asks the engine (`ExternalRenderer::pick_id`) and
//! falls back to the texels; a capture reads the texels, or the image back.

use crate::gpu::{IdHit, IdPick};
use anyhow::{Context, Result};
use ara_gpu::{
    Access, Barrier, Buffer, Device, GpuDevice, Image, Queue, Resource, commands::Command, vk,
};
use ara_runtime::extract::RenderFrame;
use std::time::Duration;

/// The frame's channels: draw `n` is channel `n + 1`, then each resident terrain tile (sorted by
/// coordinate: the bridge numbers them alike), so a click or hover on the ground names its tile
/// and the channel map names its Terrain.
pub(super) fn entities(frame: &RenderFrame) -> Vec<(u64, String)> {
    let mut tiles = frame
        .terrain
        .as_ref()
        .map_or_else(Vec::new, |terrain| terrain.tiles.clone());
    tiles.sort();
    tiles.dedup();
    frame
        .draws
        .iter()
        .map(|draw| (draw.entity, draw.label.clone()))
        .chain(tiles.iter().map(|tile| {
            (
                ara_runtime::terrain::tile_render_id(tile.x, tile.z),
                format!("terrain tile {},{}", tile.x, tile.z),
            )
        }))
        .collect()
}

/// The Scene's ids on the presenter's device: the channels, and when the engine delivered CPU
/// texels, those texels (what a click and a capture read, bit for bit) and a GPU copy for the
/// overlays to sample. An engine that delivers its id image instead (`ExternalFrame::id`) needs
/// no copy here: the host binds that image.
#[derive(Default)]
pub(super) struct Ids {
    entities: Vec<(u64, String)>,
    texels: Vec<u8>,
    size: [u32; 2],
    target: Option<Target>,
    /// A frame has been seen: the channels are meaningful.
    ready: bool,
}

/// The packed id image (RGBA8 UNORM, one texel per output pixel) and its staging buffer.
struct Target {
    image: Image,
    staging: Buffer,
    size: [u32; 2],
    /// Never written: the first upload starts from an undefined layout.
    fresh: bool,
}

impl Ids {
    /// Brings the channels up to `frame` seen at `size`, and the texels to `delivered` (the
    /// engine's `id_texels`, when it has them): uploads them when they changed. Uploads outside
    /// any deferred completion scope: it waits for the copy.
    pub(super) fn update(
        &mut self,
        gpu: &mut GpuDevice,
        frame: &RenderFrame,
        size: [u32; 2],
        delivered: Option<Vec<u8>>,
    ) -> Result<()> {
        self.entities = entities(frame);
        self.ready = true;
        let delivered = delivered.filter(|t| t.len() == size[0] as usize * size[1] as usize * 4);
        let Some(texels) = delivered else {
            // No texels this frame: the engine's image (if any) is the ids.
            self.texels.clear();
            return self.release(gpu);
        };
        // A converged still frame delivers the same ids again: nothing to upload.
        if self.size == size && self.target.is_some() && texels == self.texels {
            return Ok(());
        }
        self.texels = texels;
        self.size = size;
        if self.target.as_ref().is_some_and(|t| t.size != size) {
            self.release(gpu)?;
        }
        if self.target.is_none() {
            self.target = Some(Target::new(gpu, size)?);
        }
        let target = self
            .target
            .as_mut()
            .context("the id image was created above")?;
        crate::studio::trace::time("scene ids upload", Duration::ZERO, || {
            target.upload(gpu, &self.texels)
        })
    }

    /// The id image the overlays sample when the engine delivered texels.
    pub(super) fn image(&self) -> Option<Image> {
        self.target.as_ref().map(|t| t.image)
    }

    /// The ids as the click and the channel map read them, once a frame has been seen.
    pub(super) fn host(&self) -> Option<IdHost<'_>> {
        self.ready.then_some(IdHost {
            entities: &self.entities,
            texels: &self.texels,
            size: self.size,
            front: None,
        })
    }

    /// Hands the image and its staging buffer back to `gpu`.
    pub(super) fn release(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        if let Some(target) = self.target.take() {
            gpu.retire(Resource::Image(target.image))?;
            gpu.retire(Resource::Buffer(target.staging))?;
            gpu.collect()?;
        }
        Ok(())
    }
}

impl Target {
    fn new(gpu: &mut GpuDevice, size: [u32; 2]) -> Result<Self> {
        let image = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::SAMPLED,
        )?;
        let staging = gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 4,
            vk::BufferUsageFlags::TRANSFER_SRC,
            true,
        )?;
        Ok(Self {
            image,
            staging,
            size,
            fresh: true,
        })
    }

    /// Copies `rgba8` into the image and waits; the image ends in `CopySource`, where the
    /// UI adapter expects an external image.
    fn upload(&mut self, gpu: &mut GpuDevice, rgba8: &[u8]) -> Result<()> {
        // Every earlier use of the staging buffer was resolved below.
        gpu.upload(self.staging, rgba8)?;
        let mut commands = gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("Scene.IdUpload".into()));
        commands.barriers([Barrier {
            resource: Resource::Image(self.image),
            before: if self.fresh {
                Access::Undefined
            } else {
                Access::CopySource
            },
            after: Access::CopyDestination,
            handoff: None,
        }]);
        commands.push(Command::UploadImage {
            source: self.staging,
            destination: self.image,
        });
        commands.barriers([Barrier {
            resource: Resource::Image(self.image),
            before: Access::CopyDestination,
            after: Access::CopySource,
            handoff: None,
        }]);
        commands.push(Command::EndPass);
        let point = gpu.submit(commands.finish(), &[])?;
        gpu.resolve(point)?;
        // The UI samples this image on its own queue, like the colour copy
        // (`PresentationImage::copy`).
        gpu.publish_submission_to_ui(point)?;
        self.fresh = false;
        Ok(())
    }
}

/// The Scene's ids borrowed for a click or a capture: the channel list and the texels, if the
/// engine delivered any.
pub(in crate::studio) struct IdHost<'a> {
    /// Draw `n` is channel `n + 1`.
    pub entities: &'a [(u64, String)],
    texels: &'a [u8],
    size: [u32; 2],
    /// The engine's own answer for the click, when it gave one (`ExternalRenderer::pick_id`).
    front: Option<u32>,
}

impl IdHost<'_> {
    /// The click's answer from the engine's id pass: it sees through glass, which the
    /// opaque ids the overlays read do not.
    pub fn with_front(mut self, front: Option<u32>) -> Self {
        self.front = front;
        self
    }

    /// The packed texels (RGBA8, rows top first), when the engine delivered them by CPU copy.
    pub fn texels(&self) -> Option<&[u8]> {
        (!self.texels.is_empty()).then_some(self.texels)
    }
}

impl IdPick for IdHost<'_> {
    fn id_at(&mut self, [x, y]: [u32; 2]) -> Result<IdHit> {
        if let Some(id) = self.front {
            return Ok(IdHit::from_packed(id.to_le_bytes()));
        }
        anyhow::ensure!(
            !self.texels.is_empty(),
            "the engine delivered no id texels and did not answer the click"
        );
        anyhow::ensure!(
            x < self.size[0] && y < self.size[1],
            "pixel ({x}, {y}) is outside the {:?} id image",
            self.size
        );
        let at = (y as usize * self.size[0] as usize + x as usize) * 4;
        let texel = self
            .texels
            .get(at..at + 4)
            .context("the id image is smaller than its size")?;
        Ok(IdHit::from_packed([texel[0], texel[1], texel[2], texel[3]]))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Tests assert each step at its call site.
mod tests {
    use super::*;
    use ara_runtime::{
        components::MeshShape,
        extract::{Draw, Geometry},
    };
    use glam34::Mat4;

    fn draw(entity: u64) -> Draw {
        Draw {
            entity,
            address: format!("e{entity}"),
            label: format!("E{entity}"),
            geometry: Geometry::Shape {
                shape: MeshShape::Box,
                size: [1.; 3],
            },
            world: Mat4::IDENTITY,
            color: [1.; 4],
            roughness: 0.5,
            metallic: 0.,
            emissive: [0.; 3],
            emissive_factor: None,
            material: None,
            skin: None,
            casts_shadows: true,
            receives_decals: true,
            paint: None,
        }
    }

    /// Draw `n` is channel `n + 1`: the channel list is the draws in frame order.
    #[test]
    fn the_channels_are_the_draws_in_frame_order() {
        let mut frame = RenderFrame::default();
        frame.draws = vec![draw(7), draw(3), draw(9)];
        assert_eq!(
            entities(&frame),
            vec![
                (7, "E7".to_owned()),
                (3, "E3".to_owned()),
                (9, "E9".to_owned())
            ]
        );
    }

    fn host<'a>(texels: &'a [u8], size: [u32; 2]) -> IdHost<'a> {
        IdHost {
            entities: &[],
            texels,
            size,
            front: None,
        }
    }

    /// Texels are little-endian packed ids, rows top first; the reserved values read as
    /// nothing and as a surface without an id.
    #[test]
    fn a_click_reads_the_packed_texel_under_it() {
        let texels = [
            1, 0, 0, 0, // (0, 0): id 1
            0x01, 0x02, 0x03, 0x04, // (1, 0): id 0x04030201
            0, 0, 0, 0, // (0, 1): nothing
            0xFF, 0xFF, 0xFF, 0xFF, // (1, 1): a surface without an id
        ];
        let mut host = host(&texels, [2, 2]);
        assert_eq!(host.id_at([0, 0]).unwrap(), IdHit::Id(1));
        assert_eq!(host.id_at([1, 0]).unwrap(), IdHit::Id(0x0403_0201));
        assert_eq!(host.id_at([0, 1]).unwrap(), IdHit::Nothing);
        assert_eq!(host.id_at([1, 1]).unwrap(), IdHit::Unselectable);
        assert!(host.id_at([2, 0]).is_err(), "outside the image");
    }

    /// The engine's answer for a click wins over the texels; without either, the click errors
    /// rather than reading as an empty click that would deselect.
    #[test]
    fn the_engines_answer_wins_and_no_ids_is_an_error() {
        let texels = [1, 0, 0, 0];
        let mut asked = host(&texels, [1, 1]).with_front(Some(5));
        assert_eq!(asked.id_at([0, 0]).unwrap(), IdHit::Id(5));
        assert!(host(&[], [1, 1]).id_at([0, 0]).is_err());
        assert!(host(&[], [1, 1]).texels().is_none());
        assert!(host(&texels, [1, 1]).texels().is_some());
    }
}
