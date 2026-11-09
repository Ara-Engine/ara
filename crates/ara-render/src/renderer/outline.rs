//! Editor selection outlines drawn into the Scene frame, as Unreal draws them: the
//! selected instances draw again into the outline mask (`OUTLINE`, render size), once
//! without a depth test, so parts behind other geometry count, then against the frame's
//! depth, marking the parts in view (native-raster.slang `nativeOutlineFragment`). After
//! post, `outlineCompositeMain` outlines each group's whole silhouette over the display
//! output: full strength where it is in view, faint where it is hidden. Outline and image
//! come from the same frame, so they never drift apart while frames lag the camera.
use super::*;
use ara_gpu::{
    RootConstants,
    vulkan::{Attachment, Command, Recorder},
};

/// Bytes of the colour table at the start of the native outline buffer: 128 RGBA
/// floats, group `n` at `n - 1`; the instance rows follow it.
pub(super) const COLOURS: usize = 128 * 16;

/// Selection outlines for the following Scene frames (`Renderer::set_outlines`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outlines {
    /// Render channels (an `entities()` index plus one, as the entity-ID image holds
    /// them) and the outline group (1..=127) each belongs to. Channels of one group
    /// outline as one shape.
    pub channels: Vec<(u32, u8)>,
    /// Outline colour per group (`colours[group - 1]`), display-space (sRGB) RGBA.
    pub colours: Vec<[f32; 4]>,
    /// Band width on each side of the silhouette, in display pixels.
    pub width: f32,
    /// Opacity of the outline where the group's surface is hidden behind other geometry.
    pub hidden: f32,
}

impl Outlines {
    /// Whether anything outlines: channels in a group with a colour.
    pub(super) fn any(&self) -> bool {
        self.width > 0.
            && self
                .channels
                .iter()
                .any(|&(_, group)| group > 0 && usize::from(group) <= self.colours.len())
    }
}

impl Renderer {
    /// Outline these channels in the following Scene frames, drawn after post into the
    /// display output (the primary view only; captures and secondary views draw none).
    /// An empty set draws nothing and costs nothing.
    pub fn set_outlines(&mut self, outlines: Outlines) {
        self.outlines = outlines;
    }

    /// Records the outline mask: the selected rows without, then with, the depth test.
    pub(super) fn record_outline_mask(&self, recorder: &mut Recorder, root: RootConstants) {
        recorder.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: self.targets.images[OUTLINE],
                clear: Some([0.; 4]),
            }],
            depth: Some(Attachment {
                image: self.targets.images[DEPTH],
                clear: None,
            }),
            size: self.render,
        });
        if let Some(native) = &self.native {
            native.record_outlines(recorder, root);
        }
        recorder.push(Command::EndRendering);
    }

    /// Records the outline composite over the display output.
    pub(super) fn record_outline_composite(&self, recorder: &mut Recorder) {
        let Some(buffer) = self.native.as_ref().map(native::Native::outline_index) else {
            return;
        };
        recorder.push(Command::Pipeline(self.pipelines.outline_composite));
        let mut words = [0; 8];
        words[0] = self.display_frame_index;
        words[1] = self.targets.sampled[OUTLINE];
        words[5] = self.outlines.width.to_bits();
        words[6] = self.outlines.hidden.clamp(0., 1.).to_bits();
        words[7] = buffer;
        recorder.push(Command::Constants(RootConstants { words }));
        recorder.push(Command::Dispatch([
            self.size[0].div_ceil(8),
            self.size[1].div_ceil(8),
            1,
        ]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_need_a_coloured_group_and_a_band() {
        let mut outlines = Outlines {
            channels: vec![(3, 1)],
            colours: vec![[1., 0.5, 0., 1.]],
            width: 1.5,
            hidden: 0.4,
        };
        assert!(outlines.any());
        outlines.channels = vec![(3, 2)];
        assert!(!outlines.any(), "group 2 has no colour");
        outlines.channels = vec![(3, 1)];
        outlines.width = 0.;
        assert!(!outlines.any());
    }
}
