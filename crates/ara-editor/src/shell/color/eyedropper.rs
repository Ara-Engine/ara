//! The picker's eyedropper. Inside the editor window it samples the rendered frame
//! under the pointer (the window owner reads back [`LOUPE`]² pixels around it after
//! each frame, which the loupe magnifies); outside it, the owner samples the screen
//! (GDI on Windows). The sample previews live; a click (or releasing a drag that
//! started on the eyedropper button) applies it as one transaction; Escape or a right
//! click restores the colour from before.
use super::*;

/// The side of the pixel block the loupe magnifies; the sample is its centre.
pub(crate) const LOUPE: u32 = 11;

/// An eyedropper that is picking.
pub(super) struct Eyedropper {
    /// The colour before picking started, restored when it is cancelled.
    pub(super) before: [f64; 4],
    /// Held from its button: the release picks. Otherwise the next click picks.
    pub(super) held: bool,
    /// The magnified pixels (display sRGB, row-major, `None` outside the frame) and
    /// where they were sampled, while the pointer is over the editor window.
    pub(super) loupe: Option<(Vec<Option<[u8; 4]>>, [f32; 2])>,
}

impl Shell {
    /// Whether the eyedropper is picking (the cursor becomes a crosshair).
    pub fn eyedropper_active(&self) -> bool {
        self.color_picker
            .as_ref()
            .is_some_and(|p| p.eyedropper.is_some())
    }
    /// Whether the eyedropper is held down from its button (releasing it picks).
    pub fn eyedropper_held(&self) -> bool {
        self.color_picker
            .as_ref()
            .and_then(|p| p.eyedropper.as_ref())
            .is_some_and(|e| e.held)
    }
    /// Start picking; `held` when pressed on the button (a drag), else armed by a click.
    pub(super) fn arm_eyedropper(&mut self, held: bool) {
        if let Some(picker) = &mut self.color_picker {
            let before = picker
                .eyedropper
                .as_ref()
                .map_or_else(|| picker.value(), |e| e.before);
            picker.drag = None;
            picker.eyedropper = Some(Eyedropper {
                before,
                held,
                loupe: None,
            });
        }
    }
    /// The logical point to sample in the editor's own frame while picking, and the
    /// side of the pixel block to read around it; `None` when the pointer is outside
    /// the window (sample the screen instead).
    pub fn eyedropper_point(&self) -> Option<([f32; 2], u32)> {
        let [x, y] = self.pointer;
        (self.eyedropper_active()
            && x >= 0.
            && y >= 0.
            && x < self.ui.size[0]
            && y < self.ui.size[1])
            .then_some((self.pointer, LOUPE))
    }
    /// Pixels read back around the sampled point: `block` (display sRGB) holds the
    /// pixel sampled at `at` at `centre`. Previews that pixel and fills the loupe;
    /// true when either changed.
    pub fn eyedropper_sample(
        &mut self,
        block: &image::RgbaImage,
        centre: [u32; 2],
        at: [f32; 2],
    ) -> ara_ui::Result<bool> {
        let half = (LOUPE / 2) as i64;
        let pixels: Vec<Option<[u8; 4]>> = (0..LOUPE as i64 * LOUPE as i64)
            .map(|n| {
                let x = i64::from(centre[0]) + n % LOUPE as i64 - half;
                let y = i64::from(centre[1]) + n / LOUPE as i64 - half;
                (x >= 0 && y >= 0 && x < i64::from(block.width()) && y < i64::from(block.height()))
                    .then(|| block.get_pixel(x as u32, y as u32).0)
            })
            .collect();
        let Some(picker) = &mut self.color_picker else {
            return Ok(false);
        };
        let Some(eyedropper) = &mut picker.eyedropper else {
            return Ok(false);
        };
        let changed = eyedropper
            .loupe
            .as_ref()
            .is_none_or(|(old, old_at)| *old != pixels || *old_at != at);
        let sample = pixels[pixels.len() / 2];
        eyedropper.loupe = Some((pixels, at));
        if let Some([r, g, b, _]) = sample {
            self.preview_sample([r, g, b])?;
        }
        if changed && let Some(i) = self.ui.find("color-picker") {
            self.ui.nodes[i].dirty.paint = true;
        }
        Ok(changed)
    }
    /// A screen pixel outside the editor window (display sRGB): previews it; true when
    /// the preview changed.
    pub fn eyedropper_screen(&mut self, rgb: [u8; 3]) -> ara_ui::Result<bool> {
        let Some(eyedropper) = self
            .color_picker
            .as_mut()
            .and_then(|p| p.eyedropper.as_mut())
        else {
            return Ok(false);
        };
        let had_loupe = eyedropper.loupe.take().is_some();
        Ok(self.preview_sample(rgb)? || had_loupe)
    }
    /// Show a sampled display colour as the edited one (alpha and intensity stay).
    fn preview_sample(&mut self, rgb: [u8; 3]) -> ara_ui::Result<bool> {
        let Some(picker) = &mut self.color_picker else {
            return Ok(false);
        };
        let before = picker.hsv;
        picker.set_display(rgb.map(|c| f64::from(c) / 255.));
        picker.kelvin = None;
        if picker.hsv == before {
            return Ok(false);
        }
        self.preview_picker()?;
        Ok(true)
    }
    /// Apply the sampled colour as one transaction and stop picking.
    pub fn pick_eyedropper(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let Some(picker) = &mut self.color_picker else {
            return Ok(());
        };
        if picker.eyedropper.take().is_none() {
            return Ok(());
        }
        self.apply_color(editor, "Pick colour from screen")?;
        self.rebuild(editor)
    }
    /// Stop picking and restore the colour from before.
    pub fn cancel_eyedropper(&mut self) {
        if let Some(picker) = &mut self.color_picker
            && let Some(eyedropper) = picker.eyedropper.take()
        {
            picker.set_linear(eyedropper.before);
            self.drafts.remove(&picker.marker());
            if let Some(i) = self.ui.find("color-picker") {
                self.ui.nodes[i].dirty.paint = true;
            }
        }
    }
}
