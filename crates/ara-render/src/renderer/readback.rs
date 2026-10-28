//! Host copies belong to captures and explicit picks, never ordinary live frames.
use super::*;
use ara_gpu::{Barrier, vulkan::Command};

impl Renderer {
    pub(super) fn readback(&mut self, color: bool) -> Result<()> {
        let channels = [
            (OUTPUT, self.targets.read_color),
            (ID, self.targets.read_ids),
        ];
        let channels = &channels[usize::from(!color)..];
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("CaptureReadback".into()));
        for &(image, destination) in channels {
            ensure!(
                self.targets.states[image].access == Access::CopySource
                    && self.targets.states[image].queue == Queue::Graphics,
                "Readback requires a completed render frame"
            );
            commands.barriers([Barrier {
                resource: Resource::Buffer(destination),
                before: Access::HostRead,
                after: Access::CopyDestination,
                handoff: None,
            }]);
            commands.push(Command::ReadImage {
                source: self.targets.images[image],
                destination,
            });
            commands.barriers([Barrier {
                resource: Resource::Buffer(destination),
                before: Access::CopyDestination,
                after: Access::HostRead,
                handoff: None,
            }]);
        }
        commands.push(Command::EndPass);
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.last_timings.extend(self.gpu.resolve(point)?);
        Ok(())
    }

    /// The completed overdraw frame's fragment counts (render size, R16F), through
    /// the ID readback buffer; `None` unless the frame drew the overdraw view.
    pub(super) fn read_overdraw(&mut self) -> Result<Option<crate::capture::OverdrawStats>> {
        if !self.look.frame_overdraw {
            return Ok(None);
        }
        ensure!(
            self.targets.states[WIRE].access == Access::CopySource,
            "Overdraw readback requires a completed overdraw frame"
        );
        let destination = self.targets.read_ids;
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.push(Command::BeginPass("OverdrawReadback".into()));
        commands.barriers([Barrier {
            resource: Resource::Buffer(destination),
            before: Access::HostRead,
            after: Access::CopyDestination,
            handoff: None,
        }]);
        commands.push(Command::ReadImage {
            source: self.targets.images[WIRE],
            destination,
        });
        commands.barriers([Barrier {
            resource: Resource::Buffer(destination),
            before: Access::CopyDestination,
            after: Access::HostRead,
            handoff: None,
        }]);
        commands.push(Command::EndPass);
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.gpu.resolve(point)?;
        let pixels = self.render[0] as usize * self.render[1] as usize;
        let bytes = self.gpu.read(destination)?;
        let counts = bytes[..pixels * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|h| half_to_f32(u16::from_le_bytes(*h)).round() as u32);
        Ok(Some(crate::capture::OverdrawStats::of(counts)))
    }
}

/// An IEEE half (the count target's R16F) as f32; counts are small whole numbers.
fn half_to_f32(h: u16) -> f32 {
    let exponent = i32::from((h >> 10) & 0x1f);
    let mantissa = f32::from(h & 0x3ff);
    let magnitude = match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1. + mantissa / 1024.) * 2f32.powi(e - 15),
    };
    if h & 0x8000 != 0 {
        -magnitude
    } else {
        magnitude
    }
}
