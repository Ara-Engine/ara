//! Editor extraction updates retain the device, offscreen targets, textures,
//! environment and last good geometry.
use super::upload::upload_geometry;
use super::*;
use crate::scene::Scene;

impl Renderer {
    /// Atomically replace extracted geometry, materials and lights on the existing device.
    /// Texture/environment inputs must match the uploaded scene (use `replace_scene` to
    /// change them). A rejected or failed update retains the last good geometry; targets
    /// and device identities never change here.
    pub fn update_geometry(&mut self, scene: Scene) -> Result<()> {
        ensure!(
            super::hash::static_hash(&scene) == self.content.static_hash,
            "Texture/environment replacement requires replace_scene"
        );
        ensure!(
            scene.sun_direction.is_finite()
                && scene.sun_direction.length_squared() > 0.001
                && scene.sun_lux.is_finite(),
            "Invalid sun"
        );
        let hash = super::hash::scene_hash(&scene);
        let geometry = upload_geometry(
            &mut self.gpu,
            scene.materials,
            scene.meshes,
            scene.lights,
            &self.content.texture_indices,
        )?;
        let content = &mut self.content;
        let old_meshes = std::mem::replace(&mut content.meshes, geometry.meshes);
        let old_owned = std::mem::replace(&mut content.geometry_owned, geometry.owned);
        content.geometry_inputs = geometry.inputs;
        content.material_index = geometry.material_index;
        content.lights = geometry.lights;
        content.light_buffer = geometry.light_buffer;
        content.light_index = geometry.light_index;
        content.compute_lights = geometry.compute_lights;
        content.compute_light_index = geometry.compute_light_index;
        content.entity_names = geometry.entity_names;
        content.sun_direction = scene.sun_direction.normalize();
        content.sun_lux = scene.sun_lux;
        content.base_hash = hash;
        content.revision = 0;
        self.targets.history = None;
        for resource in old_owned {
            self.gpu.retire(resource)?;
        }
        for mesh in old_meshes {
            self.gpu.retire(Resource::Buffer(mesh.vertices))?;
            self.gpu.retire(Resource::Buffer(mesh.indices))?;
        }
        self.gpu.collect()?;
        Ok(())
    }

    /// Pick from the last completed frame's entity-ID channel, without rendering or PNG encoding.
    pub fn entity_at(&self, pixel: [u32; 2]) -> Result<Option<u64>> {
        ensure!(
            pixel[0] < self.size[0] && pixel[1] < self.size[1],
            "Pick is outside the viewport"
        );
        let bytes = self.gpu.read(self.targets.read_ids)?;
        let index = (pixel[1] as usize * self.size[0] as usize + pixel[0] as usize) * 4;
        let id = f32::from_le_bytes(bytes[index..index + 4].try_into()?) as usize;
        Ok(id
            .checked_sub(1)
            .and_then(|i| self.content.entity_names.get(i))
            .map(|(id, _)| *id))
    }
}
