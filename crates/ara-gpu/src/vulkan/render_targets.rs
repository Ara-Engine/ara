//! Validation and dynamic rendering for one mip and array slice per attachment.
use super::*;
use crate::{
    attachments::{Load, Ops},
    rendering::{RenderAttachment, RenderTarget},
    texture::SampleCount,
};

impl VulkanDevice {
    pub(super) fn validate_render_targets(
        &self,
        colors: &[RenderAttachment],
        depth: Option<RenderAttachment>,
        resolves: &[Option<RenderTarget>],
        size: [u32; 2],
    ) -> Result<(Vec<vk::Format>, Option<vk::Format>, SampleCount)> {
        if colors.len() > 8 || (!resolves.is_empty() && resolves.len() != colors.len()) {
            return Err(GpuError::Contract(
                "Invalid color/resolve target count".into(),
            ));
        }
        let mut samples = None;
        let mut targets = Vec::new();
        for (a, is_depth) in colors
            .iter()
            .map(|a| (a, false))
            .chain(depth.as_ref().map(|a| (a, true)))
        {
            let raw = self.images.get(a.target.image)?.raw;
            let extent = a.target.extent(raw.shape, [raw.width, raw.height])?;
            if (0..2).any(|i| size[i] > extent[i]) {
                return Err(GpuError::Contract("Attachment extent too small".into()));
            }
            if samples.is_some_and(|s| s != raw.shape.samples) {
                return Err(GpuError::Contract("Attachment sample counts differ".into()));
            }
            samples = Some(raw.shape.samples);
            let (aspect, usage) = if is_depth {
                (
                    vk::ImageAspectFlags::DEPTH,
                    vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                )
            } else {
                (
                    vk::ImageAspectFlags::COLOR,
                    vk::ImageUsageFlags::COLOR_ATTACHMENT,
                )
            };
            if raw.aspect != aspect || !raw.usage.contains(usage) {
                return Err(GpuError::Contract(
                    "Invalid attachment usage or aspect".into(),
                ));
            }
            if targets.contains(&a.target) {
                return Err(GpuError::Contract(
                    "Rendering attachment subresources overlap".into(),
                ));
            }
            targets.push(a.target);
        }
        for (a, target) in colors.iter().zip(resolves) {
            if let Some(target) = target {
                let src = self.images.get(a.target.image)?.raw;
                let dst = self.images.get(target.image)?.raw;
                if src.shape.samples != SampleCount::Four
                    || dst.shape.samples != SampleCount::One
                    || src.format != dst.format
                    || a.target.extent(src.shape, [src.width, src.height])?
                        != target.extent(dst.shape, [dst.width, dst.height])?
                    || !dst.usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                    || targets.contains(target)
                {
                    return Err(GpuError::Contract("Invalid in-pass color resolve".into()));
                }
                targets.push(*target);
            }
        }
        let formats = colors
            .iter()
            .map(|a| self.images.get(a.target.image).map(|i| i.raw.format))
            .collect::<Result<Vec<_>>>()?;
        let depth = depth
            .map(|a| self.images.get(a.target.image).map(|i| i.raw.format))
            .transpose()?;
        Ok((formats, depth, samples.unwrap_or_default()))
    }

    pub(super) fn encode_render_targets(
        &self,
        cmd: vk::CommandBuffer,
        colors: &[RenderAttachment],
        depth: Option<RenderAttachment>,
        resolves: &[Option<RenderTarget>],
        size: [u32; 2],
        ops: &[Ops],
    ) -> Result<()> {
        let attachment = |a: &RenderAttachment,
                          is_depth: bool,
                          ops: Ops,
                          resolve: Option<RenderTarget>|
         -> Result<_> {
            let raw = self.images.get(a.target.image)?.raw;
            let clear = a.clear.unwrap_or([0.; 4]);
            let mut info = vk::RenderingAttachmentInfo::default()
                .image_view(self.render_view(a.target)?)
                .image_layout(if is_depth {
                    vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL
                } else {
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
                })
                .load_op(match ops.load {
                    Load::Clear => vk::AttachmentLoadOp::CLEAR,
                    Load::Keep => vk::AttachmentLoadOp::LOAD,
                    Load::DontCare => vk::AttachmentLoadOp::DONT_CARE,
                })
                .store_op(if ops.store {
                    vk::AttachmentStoreOp::STORE
                } else {
                    vk::AttachmentStoreOp::DONT_CARE
                })
                .clear_value(if is_depth {
                    vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: clear[0],
                            stencil: 0,
                        },
                    }
                } else {
                    vk::ClearValue {
                        color: vk::ClearColorValue { float32: clear },
                    }
                });
            if let Some(target) = resolve {
                let integer = matches!(
                    raw.format,
                    vk::Format::R32_UINT
                        | vk::Format::R32G32_UINT
                        | vk::Format::R32G32B32A32_UINT
                        | vk::Format::R32_SINT
                        | vk::Format::R32G32_SINT
                        | vk::Format::R32G32B32A32_SINT
                );
                info = info
                    .resolve_mode(if integer {
                        vk::ResolveModeFlags::SAMPLE_ZERO
                    } else {
                        vk::ResolveModeFlags::AVERAGE
                    })
                    .resolve_image_view(self.render_view(target)?)
                    .resolve_image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
            }
            Ok(info)
        };
        let depth = depth
            .map(|a| attachment(&a, true, ops[colors.len()], None))
            .transpose()?;
        let colors = colors
            .iter()
            .zip(ops)
            .enumerate()
            .map(|(i, (a, ops))| attachment(a, false, *ops, resolves.get(i).copied().flatten()))
            .collect::<Result<Vec<_>>>()?;
        let mut info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: vk::Extent2D {
                    width: size[0],
                    height: size[1],
                },
            })
            .layer_count(1)
            .color_attachments(&colors);
        if let Some(ref depth) = depth {
            info = info.depth_attachment(depth);
        }
        // SAFETY: validation checked all subresources; cached views and these attachment arrays live through recording.
        unsafe {
            self.context.device.cmd_begin_rendering(cmd, &info);
            super::encode::viewport(&self.context.device, cmd, [0; 2], size);
        }
        Ok(())
    }
}
