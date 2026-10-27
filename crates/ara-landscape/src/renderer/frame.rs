use super::*;
impl Landscape {
    /// Replaces any of grass/rock/soil/snow sampling with a shared virtual
    /// texture ID. u32::MAX retains that layer's classic texture. Submit the
    /// cache's begin_frame before this view and end_frame after all consumers.
    pub fn bind_virtual_textures(
        &mut self,
        binding: Option<(ara_texture_stream::Binding, [u32; 4])>,
    ) -> Result<()> {
        if let Some((b, layers)) = binding {
            ensure!(
                layers
                    .iter()
                    .all(|id| *id == u32::MAX || *id < b.texture_count())
                    && b.tracking[2..] == self.size,
                "virtual landscape texture ID or viewport dimensions out of range"
            );
        }
        self.virtual_textures = binding;
        Ok(())
    }
    /// Export images for host composition and fidelity/lighting integration.
    pub fn outputs(&self) -> Outputs {
        Outputs {
            color: self.color,
            depth: self.depth,
            shadow: self.shadow,
            hzb: self.hzb,
        }
    }
    /// Number of persistent input instances; visible counts stay on the GPU.
    pub fn instance_count(&self) -> u32 {
        self.count
    }
    /// Detailed page count stays fixed at 256, with an always-resident coarse fallback.
    pub fn detail_page_capacity(&self) -> u32 {
        PAGE_CAPACITY
    }
    /// Stage one completed tile at a user-permitted boundary; at most 13 KiB of
    /// sample data per frame. This performs no file I/O.
    /// Caller must first reject stale revisions and finish this frame slot's GPU use.
    pub fn queue_tile(
        &mut self,
        gpu: &GpuDevice,
        coord: ara_terrain::TileCoord,
        tile: &ara_terrain::Tile,
        allowed: bool,
    ) -> Result<bool> {
        if !allowed || self.pending_uploads as usize >= UPLOAD_BUDGET {
            return Ok(false);
        }
        // Publish one tile per frame to keep page-table replacement and payload
        // writes disjoint even when the cache reuses an evicted slot immediately.
        if self.pending_uploads != 0 {
            return Ok(false);
        }
        let coord = self.space.local_tile(coord)?;
        self.upload_words.clear();
        let old_next = self.pages.next;
        let slot = self
            .pages
            .owners
            .iter()
            .position(|c| *c == Some(coord))
            .unwrap_or(old_next);
        let old_owner = self.pages.owners[slot];
        self.pages.upload(coord, tile, &mut self.upload_words)?;
        if let Err(e) = gpu.upload(self.upload, bytemuck::cast_slice(&self.upload_words)) {
            self.pages.owners[slot] = old_owner;
            self.pages.next = old_next;
            return Err(e.into());
        }
        self.pending_uploads = 1;
        Ok(true)
    }
    /// Upload explicit frame state after this frame slot's previous submission completes.
    pub fn update(&self, gpu: &GpuDevice, view: &View) -> Result<()> {
        ensure!(
            view.eye.is_finite()
                && view.target.is_finite()
                && view.sun.is_finite()
                && view.sun_color.is_finite()
                && view.dt.is_finite()
                && view.dt > 0.
                && view.dt <= 1.
                && view.sun.length_squared() > 0.01
                && view.eye.distance_squared(view.target) > 0.01
                && view.fov_y > 0.
                && view.fov_y < std::f32::consts::PI
                && view.near > 0.,
            "invalid landscape view"
        );
        let eye = view.eye - self.space.offset;
        let target = view.target - self.space.offset;
        ensure!(
            eye.is_finite() && target.is_finite(),
            "landscape view rebase overflow"
        );
        let forward = (target - eye).normalize();
        let camera_up = if forward.dot(Vec3::Y).abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let sun = view.sun.normalize();
        let sun_up = if sun.dot(Vec3::Y).abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let vp = Mat4::perspective_infinite_reverse_rh(
            view.fov_y,
            self.size[0] as f32 / self.size[1] as f32,
            view.near,
        ) * Mat4::look_at_rh(eye, target, camera_up);
        let center = eye + forward * 350.;
        let light_vp = Mat4::orthographic_rh(-650., 650., -650., 650., 1800., 0.1)
            * Mat4::look_at_rh(center + sun * 1200., center, sun_up);
        let frame = Frame {
            vp,
            light_vp,
            inverse_vp: vp.inverse(),
            camera: eye.extend(0.).to_array(),
            sun: sun.extend(0.).to_array(),
            sun_color: view.sun_color.extend(0.).to_array(),
            grid: [self.side, self.cells, self.tiles, self.count],
            extent: [self.size[0], self.size[1], view.tick, self.pending_uploads],
            indices: [self.slots[1], self.slots[2], self.slots[3], self.slots[4]],
            more: [self.slots[5], self.slots[6], self.slots[7], self.slots[8]],
            params: [
                self.spacing,
                view.dt * view.tick as f32,
                self.image_slots[2] as f32,
                self.image_slots[3] as f32,
            ],
            geometry: [u32::from(self.geometry.is_some()), self.geometry_slot, 0, 0],
            virtual_slots: self.virtual_textures.map_or([0; 4], |(b, _)| b.slots),
            virtual_tracking: self.virtual_textures.map_or([0; 4], |(b, _)| b.tracking),
            virtual_layers: self
                .virtual_textures
                .map_or([u32::MAX; 4], |(_, layers)| layers),
        };
        gpu.upload(self.frame, bytemuck::bytes_of(&frame))?;
        Ok(())
    }
}
