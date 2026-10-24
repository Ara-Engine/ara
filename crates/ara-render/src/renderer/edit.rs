//! Editor extraction updates retain the device, offscreen targets, textures,
//! environment and last good geometry.
use super::upload::upload_geometry;
use super::*;
use crate::scene::Scene;
use sha2::{Digest, Sha256};

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
        content.material_buffer = geometry.material_buffer;
        content.material_count = geometry.material_count;
        content.material_update = None;
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

    /// Replace material parameters in their existing order without uploading geometry.
    /// Texture indices address the original scene textures. Invalid inputs retain the
    /// last good materials; changing textures or material count requires a scene update.
    pub fn update_materials(&mut self, materials: Vec<crate::scene::Material>) -> Result<()> {
        ensure!(
            materials.len() == self.content.material_count,
            "Material update changes the material count"
        );
        let digest = Sha256::digest(bytemuck::cast_slice(&materials)).into();
        let materials = super::upload::prepare_materials(materials, &self.content.texture_indices)?;
        let old = Resource::Buffer(self.content.material_buffer);
        let input = self
            .content
            .geometry_inputs
            .iter()
            .position(|(r, _)| *r == old)
            .ok_or_else(|| anyhow::anyhow!("Material input is missing"))?;
        let owned = self
            .content
            .geometry_owned
            .iter()
            .position(|r| *r == old)
            .ok_or_else(|| anyhow::anyhow!("Material ownership is missing"))?;
        let buffer = super::transfer::upload_buffer(
            &mut self.gpu,
            bytemuck::cast_slice(&materials),
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let index = match self.gpu.storage_buffer(buffer) {
            Ok(slot) => slot.index,
            Err(error) => {
                self.gpu.retire(Resource::Buffer(buffer))?;
                return Err(error.into());
            }
        };
        self.content.geometry_inputs[input].0 = Resource::Buffer(buffer);
        self.content.geometry_owned[owned] = Resource::Buffer(buffer);
        self.content.material_buffer = buffer;
        self.content.material_index = index;
        self.content.material_update = Some(digest);
        for mesh in &mut self.content.meshes {
            mesh.transparent = materials[mesh.material as usize].flags[0] == 2;
        }
        self.content.revision += 1;
        self.targets.history = None;
        self.gpu.retire(old)?;
        self.gpu.collect()?;
        Ok(())
    }

    /// Update an uploaded entity's capture label without changing its geometry.
    pub fn rename(&mut self, entity: u64, name: String) -> Result<()> {
        let (_, current) = self
            .content
            .entity_names
            .iter_mut()
            .find(|(id, _)| *id == entity)
            .ok_or_else(|| anyhow::anyhow!("Unknown mesh entity {entity}"))?;
        if *current != name {
            *current = name;
            self.content.revision += 1;
        }
        Ok(())
    }

    /// Read the last completed frame's entity-ID channel on demand, without rendering
    /// or PNG encoding. Ordinary live frames never copy pixels to host memory.
    pub fn entity_at(&mut self, pixel: [u32; 2]) -> Result<Option<u64>> {
        ensure!(
            pixel[0] < self.size[0] && pixel[1] < self.size[1],
            "Pick is outside the viewport"
        );
        if self.targets.states[ID].access == Access::Undefined {
            return Ok(None);
        }
        self.readback(false)?;
        let bytes = self.gpu.read(self.targets.read_ids)?;
        let index = (pixel[1] as usize * self.size[0] as usize + pixel[0] as usize) * 4;
        let id = f32::from_le_bytes(bytes[index..index + 4].try_into()?) as usize;
        Ok(id
            .checked_sub(1)
            .and_then(|i| self.content.entity_names.get(i))
            .map(|(id, _)| *id))
    }
}
