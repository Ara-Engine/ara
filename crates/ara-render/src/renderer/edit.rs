//! Editor extraction updates retain the device, offscreen targets and last good geometry.
use super::{
    transfer::{upload_buffer, upload_buffer_on},
    *,
};
use crate::scene::{Light, Scene, Vertex};
struct Geometry {
    meshes: Vec<Mesh>,
    graphics: Vec<(Resource, Access)>,
    compute: Vec<(Resource, Access)>,
    names: Vec<(u64, String)>,
    material: u32,
    lights: u32,
    compute_lights: u32,
    light_count: u32,
    hash: String,
    sun_direction: Vec3,
    sun_lux: Vec3,
}
impl Renderer {
    /// Atomically replace extracted geometry, materials and lights on the existing device.
    /// Texture/environment inputs must match the original scene. A rejected or failed update
    /// retains the last good geometry; targets and device identities never change here.
    pub fn update_geometry(&mut self, scene: Scene) -> Result<()> {
        ensure!(
            !scene.meshes.is_empty() && !scene.materials.is_empty(),
            "Scene has no geometry/materials"
        );
        ensure!(
            scene.meshes.len() < 16_777_216 && scene.lights.len() <= 64,
            "Baseline scene capacity exceeded"
        );
        ensure!(
            super::hash::static_hash(&scene) == self.static_scene_hash,
            "Texture/environment replacement requires the asset integration"
        );
        ensure!(
            scene.sun_direction.is_finite()
                && scene.sun_direction.length_squared() > 0.001
                && scene.sun_lux.is_finite(),
            "Invalid sun"
        );
        ensure!(
            bytemuck::cast_slice::<Light, f32>(&scene.lights)
                .iter()
                .all(|v| v.is_finite()),
            "Non-finite light"
        );
        let mut ids = std::collections::BTreeSet::new();
        for mesh in &scene.meshes {
            ensure!(ids.insert(mesh.entity), "Duplicate entity ID");
            ensure!(
                mesh.material < scene.materials.len()
                    && !mesh.vertices.is_empty()
                    && !mesh.indices.is_empty()
                    && mesh.indices.len() % 3 == 0,
                "Invalid mesh"
            );
            ensure!(
                mesh.indices
                    .iter()
                    .all(|i| (*i as usize) < mesh.vertices.len()),
                "Mesh index out of bounds"
            );
            ensure!(
                bytemuck::cast_slice::<Vertex, f32>(&mesh.vertices)
                    .iter()
                    .all(|v| v.is_finite()),
                "Non-finite mesh vertex"
            );
        }
        ensure!(
            scene.materials.iter().all(|m| m
                .textures
                .iter()
                .all(|i| (*i as usize) < self.scene_textures.len())),
            "Material texture out of bounds"
        );
        ensure!(
            scene.materials.iter().all(|m| m
                .base_color
                .iter()
                .chain(&m.emissive_roughness)
                .chain(&m.surface)
                .all(|v| v.is_finite())),
            "Non-finite material"
        );
        let hash = super::hash::scene_hash(&scene);
        let mut resources = Vec::new();
        let prepared = (|| -> Result<Geometry> {
            let mut materials = scene.materials;
            for material in &mut materials {
                for index in &mut material.textures {
                    *index = self.scene_textures[*index as usize];
                }
            }
            let material_buffer = upload_buffer(
                &mut self.gpu,
                bytemuck::cast_slice(&materials),
                vk::BufferUsageFlags::STORAGE_BUFFER,
            )?;
            resources.push(Resource::Buffer(material_buffer));
            let material = self.gpu.storage_buffer(material_buffer)?.index;
            let mut graphics = vec![(Resource::Buffer(material_buffer), Access::ShaderRead)];
            let mut meshes = Vec::new();
            let mut names = Vec::new();
            for mesh in scene.meshes {
                let centre = mesh
                    .vertices
                    .iter()
                    .map(|v| Vec3::from_array(v.position))
                    .sum::<Vec3>()
                    / mesh.vertices.len() as f32;
                let vertices = upload_buffer(
                    &mut self.gpu,
                    bytemuck::cast_slice(&mesh.vertices),
                    vk::BufferUsageFlags::VERTEX_BUFFER,
                )?;
                resources.push(Resource::Buffer(vertices));
                let indices = upload_buffer(
                    &mut self.gpu,
                    bytemuck::cast_slice(&mesh.indices),
                    vk::BufferUsageFlags::INDEX_BUFFER,
                )?;
                resources.push(Resource::Buffer(indices));
                graphics.extend([
                    (Resource::Buffer(vertices), Access::Vertex),
                    (Resource::Buffer(indices), Access::Index),
                ]);
                names.push((mesh.entity, mesh.name));
                meshes.push(Mesh {
                    vertices,
                    indices,
                    count: u32::try_from(mesh.indices.len())?,
                    material: mesh.material as u32,
                    transparent: materials[mesh.material].flags[0] == 2,
                    centre,
                });
            }
            let light_count = scene.lights.len() as u32;
            let lights = if scene.lights.is_empty() {
                vec![Light {
                    position_range: [0.; 4],
                    color_lumens: [0.; 4],
                    direction_outer: [0.; 4],
                    parameters: [0.; 4],
                }]
            } else {
                scene.lights
            };
            let light = upload_buffer(
                &mut self.gpu,
                bytemuck::cast_slice(&lights),
                vk::BufferUsageFlags::STORAGE_BUFFER,
            )?;
            resources.push(Resource::Buffer(light));
            let compute = upload_buffer_on(
                &mut self.gpu,
                bytemuck::cast_slice(&lights),
                vk::BufferUsageFlags::STORAGE_BUFFER,
                Queue::Compute,
            )?;
            resources.push(Resource::Buffer(compute));
            let lights = self.gpu.storage_buffer(light)?.index;
            let compute_lights = self.gpu.storage_buffer(compute)?.index;
            graphics.push((Resource::Buffer(light), Access::ShaderRead));
            Ok(Geometry {
                meshes,
                graphics,
                compute: vec![(Resource::Buffer(compute), Access::ShaderRead)],
                names,
                material,
                lights,
                compute_lights,
                light_count,
                hash,
                sun_direction: scene.sun_direction.normalize(),
                sun_lux: scene.sun_lux,
            })
        })();
        let geometry = match prepared {
            Ok(g) => g,
            Err(error) => {
                for resource in resources {
                    self.gpu.retire(resource)?;
                }
                self.gpu.collect()?;
                return Err(error);
            }
        };
        let old = std::mem::replace(&mut self.dynamic, resources);
        self.graphics_inputs
            .retain(|(r, _)| !matches!(r, Resource::Buffer(_)));
        self.graphics_inputs.extend(geometry.graphics);
        self.compute_inputs = geometry.compute;
        self.meshes = geometry.meshes;
        self.entity_names = geometry.names;
        self.material_index = geometry.material;
        self.light_index = geometry.lights;
        self.compute_light_index = geometry.compute_lights;
        self.light_count = geometry.light_count;
        self.sun_direction = geometry.sun_direction;
        self.sun_lux = geometry.sun_lux;
        self.scene_hash = geometry.hash;
        self.targets.history = None;
        for resource in old {
            self.gpu.retire(resource)?;
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
            .and_then(|i| self.entity_names.get(i))
            .map(|(id, _)| *id))
    }
}
