//! The shared native view feeds every raster preset and presentation target.
use super::*;
use ara_render_fidelity::presets::RasterPlan;

impl Presenter {
    pub(super) fn prepare_raster(
        &mut self,
        frame: &RenderFrame,
        plan: RasterPlan,
        resolve: impl FnOnce() -> Result<Content>,
    ) -> Result<()> {
        let look = self.frame_look(frame);
        if self.raster.is_none() {
            let mut raster = RasterView::new(&self.cache);
            bind_probes(&mut raster, self.probe_scene.as_ref(), &self.cache);
            self.raster = Some(raster);
        }
        let raster = self.raster.as_mut().context("raster view")?;
        raster.set_probe_gi(self.probe_gi);
        let update = raster.prepare(
            self.renderer.as_mut().context("raster device")?,
            frame,
            plan,
            look,
            resolve,
        )?;
        self.stats.rebuilt = update.rebuilt;
        self.stats.updated = update.updated;
        Ok(())
    }
}
