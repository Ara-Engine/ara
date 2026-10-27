//! Explicit dev readback, preserving live history and the resource's queue/access state.
use super::*;
use anyhow::Context;
use ara_gpu::{Access, Barrier, Device, Resource, commands::Command, vk};

impl Renderer {
    /// Current pre-exposure scene-linear lighting, without resetting temporal state.
    /// This explicit lab boundary allocates/readbacks once; ordinary frames do not.
    pub fn lab_radiance(&mut self) -> Result<([u32; 2], Vec<[f32; 4]>)> {
        let size = self.size();
        let history = self
            .targets
            .history
            .context("temporal history is not initialized")?;
        let image = self.targets.images[history];
        let state = self.targets.states[history];
        let exposure = self.gpu.read(self.look.state)?;
        // The temporal resolve used the preceding multiplier; exposureMain then
        // moved it into y before computing this frame's display multiplier.
        let scale = f32::from_le_bytes(exposure[4..8].try_into()?);
        ensure!(
            scale.is_finite() && scale > 0.,
            "temporal history exposure is not initialized"
        );
        let buffer = self.gpu.create_buffer(
            u64::from(size[0]) * u64::from(size[1]) * 8,
            vk::BufferUsageFlags::TRANSFER_DST,
            true,
        )?;
        let mut commands = self.gpu.begin(state.queue);
        commands.barriers([Barrier {
            resource: Resource::Image(image),
            before: state.access,
            after: Access::CopySource,
            handoff: None,
        }]);
        commands.push(Command::ReadImage {
            source: image,
            destination: buffer,
        });
        commands.barriers([
            Barrier {
                resource: Resource::Image(image),
                before: Access::CopySource,
                after: state.access,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(buffer),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            },
        ]);
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.gpu.resolve(point)?;
        let bytes = self.gpu.read(buffer);
        self.gpu.retire(Resource::Buffer(buffer))?;
        self.gpu.collect()?;
        let bytes = bytes?;
        ensure!(
            bytes.len() == size[0] as usize * size[1] as usize * 8,
            "HDR readback extent mismatch"
        );
        let pixels = bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|p| {
                std::array::from_fn(|i| {
                    let value = half(u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]));
                    if i < 3 { value / scale } else { value }
                })
            })
            .collect();
        Ok((size, pixels))
    }
}
fn half(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 31);
    let mantissa = u32::from(bits & 1023);
    if exponent == 0 {
        let value = (mantissa as f32) * (1_f32 / 16_777_216.);
        if sign == 0 { value } else { -value }
    } else if exponent == 31 {
        f32::from_bits(sign | 0x7f80_0000 | (mantissa << 13))
    } else {
        f32::from_bits(sign | ((exponent + 112) << 23) | (mantissa << 13))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hdr_half_boundaries_preserve_physical_radiance_and_nonfinite_values() {
        assert_eq!(half(0x3c00), 1.);
        assert_eq!(half(0xc000), -2.);
        assert_eq!(half(0x7bff), 65504.);
        assert_eq!(half(0x0001), 2_f32.powi(-24));
        assert_eq!(half(0x0400), 2_f32.powi(-14));
        assert!(half(0x7c00).is_infinite());
        assert!(half(0x7e00).is_nan());
    }
}
