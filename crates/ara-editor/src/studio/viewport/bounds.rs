//! World-space bounds of what the Scene view presents, read from the retained native
//! instances; no files, no GPU pixels. The Rect tool frames the selection with it.
use super::SceneView;
use crate::world::channels::Channels;
use glam::{Mat4, Quat, Vec3};

impl SceneView {
    /// The presented primitives' bounds whose entity address `keep` takes, as a box in the
    /// frame `rotation` (each primitive's own geometry box, through its world pose, into
    /// that frame): tight round an object in its own frame (the Rect tool).
    pub(crate) fn bounds_in(
        &self,
        channels: &Channels,
        keep: impl Fn(&str) -> bool,
        rotation: Quat,
    ) -> Option<[Vec3; 2]> {
        let scene = &self.prepared.as_ref()?.scene;
        let into = Mat4::from_quat(rotation.inverse());
        scene
            .instances
            .iter()
            .filter_map(|instance| {
                if !keep(channels.target_of(instance.entity)?) {
                    return None;
                }
                let local = scene.geometry.get(instance.geometry)?.bounds;
                let to = into * instance.world;
                let corners = (0..8).map(|i| {
                    to.transform_point3(Vec3::new(
                        local[i & 1].x,
                        local[(i >> 1) & 1].y,
                        local[(i >> 2) & 1].z,
                    ))
                });
                let bounds =
                    corners.fold([Vec3::MAX, Vec3::MIN], |[lo, hi], p| [lo.min(p), hi.max(p)]);
                (bounds.iter().all(|p| p.is_finite()) && bounds[0].cmple(bounds[1]).all())
                    .then_some(bounds)
            })
            .reduce(|a, b| [a[0].min(b[0]), a[1].max(b[1])])
    }
}
