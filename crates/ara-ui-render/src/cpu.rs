//! Convenience capture facade over the persistent CPU GPU-contract adapter.
use ara_ui::{Result, paint::DisplayList};
use ara_ui_text::FontAsset;
impl crate::CpuRenderer {
    /// Render an image-free display list to PNG. Use Renderer with CpuGpu for image resources.
    pub fn render(
        list: &DisplayList,
        fonts: &[FontAsset],
        width: u32,
        height: u32,
        path: &std::path::Path,
    ) -> Result<()> {
        let mut renderer = crate::gpu::Renderer::default();
        let mut cpu = crate::CpuGpu::default();
        renderer.render(
            &mut cpu,
            list,
            fonts,
            [width, height],
            crate::atlas::TextMode::Raster,
        )?;
        cpu.save(path)
    }
}
