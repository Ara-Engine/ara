use super::*;
impl VirtualShadows {
    /// Uploads light projections at a completed boundary. Changing any matrix or
    /// caster/light revision invalidates cached pages before they can be sampled.
    pub fn update(
        &mut self,
        gpu: &mut VulkanDevice,
        geometry: &GeometryView,
        lights: &[Light],
        light_position: Vec3,
        reset: bool,
    ) -> Result<()> {
        ensure!(
            !lights.is_empty() && lights.len() <= 6 && light_position.is_finite(),
            "invalid VSM projections"
        );
        if let Some(p) = self.last {
            ensure!(
                gpu.completed()?[p.queue.index()] >= p.value,
                "VSM frame still in flight"
            );
        }
        self.frame_id = self
            .frame_id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("VSM frame counter exhausted; recreate view"))?;
        let changed = geometry.residency.revision != self.geometry_revision
            || lights.len() != self.projection_count
            || lights
                .iter()
                .enumerate()
                .any(|(i, l)| l.vp != self.matrices[i] || l.revision != self.revisions[i]);
        let mut projections = [Projection::zeroed(); 6];
        for (i, l) in lights.iter().enumerate() {
            ensure!(l.vp.is_finite(), "invalid shadow matrix");
            self.matrices[i] = l.vp;
            self.revisions[i] = l.revision;
            let perspective = !matches!(l.projection, ara_vsm::Projection::Directional(_));
            projections[i] = Projection {
                vp: l.vp,
                camera_focal: [
                    light_position.x,
                    light_position.y,
                    light_position.z,
                    l.vp.row(1).truncate().length() * 8192.0,
                ],
                control: [
                    u32::from(perspective),
                    l.revision as u32,
                    (l.revision >> 32) as u32,
                    0,
                ],
            };
        }
        self.projection_count = lights.len();
        self.geometry_revision = geometry.residency.revision;
        let s = self.slots;
        let f = ShadowFrame {
            projections,
            counts: [
                lights.len() as u32,
                self.capacity,
                self.cluster_capacity,
                self.frame_id,
            ],
            extent: [self.size[0], self.size[1], u32::from(reset || changed), 0],
            slots0: [s[0], s[1], s[2], s[3]],
            slots1: [s[4], s[5], s[6], s[7]],
            slots2: [
                self.mask_slot,
                self.world_slot,
                self.normal_slot,
                self.preview_slot,
            ],
            geometry: [
                geometry.slots[0],
                geometry.slots[1],
                geometry.slots[2],
                geometry.slots[3],
            ],
            minimum: [
                geometry.min[0],
                geometry.min[1],
                geometry.min[2],
                geometry.instance_count as f32,
            ],
            scale: [
                geometry.scale[0],
                geometry.scale[1],
                geometry.scale[2],
                geometry.cluster_count as f32,
            ],
            inputs: [
                geometry.residency.table_slot,
                geometry.residency.floor,
                0,
                0,
            ],
        };
        gpu.upload(self.frame, bytemuck::bytes_of(&f))?;
        Ok(())
    }
}
