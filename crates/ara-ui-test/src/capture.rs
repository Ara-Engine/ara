use crate::{CaptureReport, Failure, Result, Tolerance, geometry, inspection, query};
use ara_ui::{Ui, paint::DisplayList};
use ara_ui_render::{
    CpuGpu,
    atlas::TextMode,
    gpu::{Image, Renderer},
};
use ara_ui_stories::{Locale, StoryView};
use ara_ui_text::{Text, TextSystem};
use std::path::{Path, PathBuf};

/// PNG bytes and versioned agent evidence for the same native frame.
pub struct Capture {
    /// Encoded physical-resolution RGBA PNG.
    pub png: Vec<u8>,
    /// Logical geometry, semantic state and actual renderer diagnostics.
    pub report: CaptureReport,
    /// Native tree included in assertion failure output.
    pub tree: String,
}
impl Capture {
    /// Persist a PNG and canonical JSON report; returns both paths.
    pub fn write(&self, directory: &Path, stem: &str) -> Result<[PathBuf; 2]> {
        validate_stem(stem)?;
        std::fs::create_dir_all(directory)?;
        let png = directory.join(format!("{stem}.png"));
        let report = directory.join(format!("{stem}.json"));
        std::fs::write(&png, &self.png)?;
        let mut db = ara_schema::SchemaDb::new();
        crate::register(&mut db);
        let value = db.canonical_value(
            &ara_schema::ValueType::Named("CaptureReport".into()),
            &ara_json::to_value(&self.report)?,
        );
        std::fs::write(&report, ara_json::to_canonical_string(&value))?;
        Ok([png, report])
    }
    /// Assert a golden with perceptual tolerance; on failure write actual/expected/diff PNGs and tree.
    pub fn compare(
        &self,
        expected: &[u8],
        tolerance: Tolerance,
        directory: &Path,
        stem: &str,
    ) -> Result<()> {
        validate_stem(stem)?;
        let comparison = crate::compare(expected, &self.png, tolerance)?;
        if comparison.pass {
            return Ok(());
        }
        std::fs::create_dir_all(directory)?;
        std::fs::write(directory.join(format!("{stem}-actual.png")), &self.png)?;
        std::fs::write(directory.join(format!("{stem}-expected.png")), expected)?;
        let diff = directory.join(format!("{stem}-diff.png"));
        std::fs::write(&diff, comparison.diff_png)?;
        let failure = Failure {
            message: format!(
                "Golden {stem} differs: SSIM loss {:.6}, perceptual error {:.6}, changed pixels {:.3}%",
                comparison.ssim_loss,
                comparison.mean_error,
                comparison.changed_fraction * 100.0
            ),
            tree: self.tree.clone(),
            diff_image: Some(diff),
        };
        std::fs::write(
            directory.join(format!("{stem}-failure.txt")),
            failure.to_string(),
        )?;
        Err(failure.into())
    }
}
fn validate_stem(stem: &str) -> Result<()> {
    if stem.is_empty()
        || !stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(
            "capture stem must contain only letters, digits, hyphens and underscores".into(),
        );
    }
    Ok(())
}

/// Persistent CPU renderer/font resources shared across captures, without a window or GPU device.
pub struct CaptureEngine {
    pub(crate) text: TextSystem,
    renderer: Renderer,
    cpu: CpuGpu,
}
impl CaptureEngine {
    /// Use Ara's pinned bundled Latin and Arabic fonts.
    pub fn bundled() -> Result<Self> {
        Self::with_fonts(TextSystem::bundled()?)
    }
    /// Supply explicit licensed fonts; a Latin priming sample fixes the preferred face at handle zero.
    pub fn with_fonts(mut text: TextSystem) -> Result<Self> {
        text.measure(&mut Text::new("Ara".into()), None, 13.0);
        Ok(Self {
            text,
            renderer: Renderer::default(),
            cpu: CpuGpu::default(),
        })
    }
    /// Register a decoded image using the source string's production runtime resource handle.
    pub fn image(&mut self, source: &str, image: Image) -> Result<()> {
        self.renderer.image(
            ara_ui::source::stable_id(source) as u32 & 0x7fff_ffff,
            image,
        )
    }
    /// Render any source document at an explicit size/DPI/locale/theme using the native runtime.
    /// Literal captions expand in pseudo-locales; `loc:key` captions resolve against `locale`.
    pub fn document(
        &mut self,
        markup: &str,
        stylesheet: &str,
        view: &StoryView,
        locale: &Locale,
    ) -> Result<Capture> {
        view.physical_size()?;
        if view.locale != locale.id {
            return Err("capture view and locale pack identifiers differ".into());
        }
        let markup = ara_ui_stories::localize_markup(markup, locale)?;
        let css = format!("{}\n{stylesheet}", ara_ui::theme::EDITOR_CSS);
        let mut ui = Ui::with_tokens(
            &markup,
            &css,
            [view.width as f32, view.height as f32],
            view.theme.native().tokens()?,
        )?;
        self.capture(&mut ui, view, locale.rtl)
    }
    /// Layout, shape and emit once, then run RTL only when the native display list was rebuilt.
    pub(crate) fn frame(&mut self, ui: &mut Ui, rtl: bool) -> Result<ara_ui::Work> {
        let work = ui.frame(&mut self.text)?;
        if rtl && work.paint_nodes > 0 {
            geometry::mirror_rtl(ui);
        }
        Ok(work)
    }
    /// Capture an owned retained surface. The caller keeps its input/fixture/clock state.
    /// `rtl` must remain constant for a mounted surface; remount when switching locale direction.
    pub fn capture(&mut self, ui: &mut Ui, view: &StoryView, rtl: bool) -> Result<Capture> {
        view.physical_size()?;
        if ui.size != [view.width as f32, view.height as f32] {
            return Err("capture view dimensions must match the retained surface".into());
        }
        let work = self.frame(ui, rtl)?;
        self.render(ui, view, &work)
    }
    fn render(&mut self, ui: &Ui, view: &StoryView, work: &ara_ui::Work) -> Result<Capture> {
        let physical: DisplayList = geometry::physical_list(&ui.display, view.dpi as f32 / 96.0);
        self.renderer.render(
            &mut self.cpu,
            &physical,
            &self.text.assets,
            view.physical_size()?,
            TextMode::Raster,
        )?;
        let mut report = inspection::inspect(ui, view, &self.renderer)?;
        report.work = ara_json::to_value(&work)?;
        Ok(Capture {
            png: self.cpu.pixmap()?.encode_png()?,
            report,
            tree: query::tree_dump(ui),
        })
    }
}
