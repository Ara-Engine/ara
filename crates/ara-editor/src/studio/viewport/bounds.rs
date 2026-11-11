//! World-space bounds of what the Scene view presents, read from the retained native
//! instances; no files, no GPU pixels. The Rect tool frames the selection with it.
use super::SceneView;
use crate::world::channels::Channels;
use glam::Vec3;

impl SceneView {
    /// World-space bounds of the presented primitives whose entity address `keep` takes.
    pub(crate) fn bounds_where(
        &self,
        channels: &Channels,
        keep: impl Fn(&str) -> bool,
    ) -> Option<[Vec3; 2]> {
        let scene = self
            .raster
            .as_ref()
            .and_then(ara_runtime_render::RasterView::scene)
            .or_else(|| self.prepared.as_ref().map(|prepared| &prepared.scene))?;
        scene
            .instances
            .iter()
            .filter_map(|instance| {
                if !keep(channels.target_of(instance.entity)?) {
                    return None;
                }
                let bounds = instance.bounds(scene.geometry.get(instance.geometry)?);
                (bounds.iter().all(|point| point.is_finite()) && bounds[0].cmple(bounds[1]).all())
                    .then_some(bounds)
            })
            .reduce(|a, b| [a[0].min(b[0]), a[1].max(b[1])])
    }
}
