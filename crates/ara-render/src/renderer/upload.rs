use super::transfer::{upload_buffer, upload_image};
use super::*;
use crate::graph::{Graph, State};
use crate::scene::{Light, Vertex};
use ara_gpu::{RootConstants, vulkan::Command};
use bytemuck::Zeroable;
use sha2::{Digest, Sha256};

/// Capacity of the host-visible light lists, matching the tiled culling limit.
pub(super) const MAX_LIGHTS: usize = 64;

/// Everything uploaded from one extraction snapshot. Replacing a scene builds a new
/// `Content` and retires the old one; the device, pipelines and targets survive.
pub(super) struct Content {
    pub meshes: Vec<Mesh>,
    /// Materials and static mesh buffers at their resting access (every graphics pass).
    pub geometry_inputs: Vec<(Resource, Access)>,
    /// Textures, environment and IBL images at their resting access.
    pub static_inputs: Vec<(Resource, Access)>,
    /// Material and light buffers; mesh buffers are retired through `meshes`.
    pub geometry_owned: Vec<Resource>,
    /// Texture, environment and IBL images.
    pub static_owned: Vec<Resource>,
    pub ibl_pipelines: [Pipeline; 2],
    /// Bindless slots of the scene textures, in scene order.
    pub texture_indices: Vec<u32>,
    /// Hash of the textures and environment (`update_geometry` must keep them).
    pub static_hash: String,
    pub material_index: u32,
    pub lights: Vec<Light>,
    pub light_buffer: Buffer,
    pub light_index: u32,
    pub compute_lights: Buffer,
    pub compute_light_index: u32,
    pub environment: u32,
    pub irradiance: u32,
    pub prefilter: u32,
    pub sun_direction: Vec3,
    pub sun_lux: Vec3,
    /// SHA-256 of the uploaded static content.
    pub base_hash: String,
    /// Bumped by every dynamic vertex/light/sun update since the upload.
    pub revision: u64,
    pub entity_names: Vec<(u64, String)>,
}

impl Renderer {
    pub(super) fn upload_scene(
        mut gpu: VulkanDevice,
        pipelines: Pipelines,
        targets: Targets,
        scene: Scene,
        size: [u32; 2],
        artifacts: &Path,
        shader_metadata: crate::capture::ShaderMetadata,
    ) -> Result<Self> {
        let frame = gpu.create_buffer(
            std::mem::size_of::<frame::Frame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        let compute_frame = gpu.create_buffer(
            std::mem::size_of::<frame::Frame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let compute_frame_index = gpu.storage_buffer(compute_frame)?.index;
        let content = upload_content(&mut gpu, scene, artifacts, &shader_metadata, compute_frame)?;
        let look = crate::look::gpu::LookGpu::new(
            &mut gpu,
            artifacts,
            &shader_metadata.content_key,
            size,
        )?;
        Ok(Self {
            gpu,
            size,
            targets,
            pipelines,
            content,
            frame,
            frame_index,
            compute_frame,
            compute_frame_index,
            previous_vp: Mat4::IDENTITY,
            frame_number: 0,
            artifacts: artifacts.to_path_buf(),
            shader_metadata,
            last_timings: Vec::new(),
            look,
        })
    }

    /// Replace every mesh, material, texture, light and the environment with a new
    /// extraction snapshot, keeping the device, pipelines and view targets. Use it for
    /// structural changes (spawns, asset reloads); per-frame motion uses `update_mesh`.
    /// On error the previous content stays live (last-good).
    pub fn replace_scene(&mut self, scene: Scene) -> Result<()> {
        let content = upload_content(
            &mut self.gpu,
            scene,
            &self.artifacts,
            &self.shader_metadata,
            self.compute_frame,
        )?;
        let old = std::mem::replace(&mut self.content, content);
        for resource in old.static_owned.into_iter().chain(old.geometry_owned) {
            self.gpu.retire(resource)?;
        }
        for mesh in old.meshes {
            self.gpu.retire(Resource::Buffer(mesh.vertices))?;
            self.gpu.retire(Resource::Buffer(mesh.indices))?;
        }
        for pipeline in old.ibl_pipelines {
            self.gpu.retire_pipeline(pipeline)?;
        }
        self.gpu.collect()?;
        self.targets.history = None;
        self.frame_number = 0;
        Ok(())
    }

    /// Replace one mesh's world-space vertices for the next frame (moving or skinned
    /// geometry). The vertex count must match the uploaded mesh; indices, material and
    /// identity are unchanged. The first call moves the mesh to host-visible storage.
    pub fn update_mesh(&mut self, entity: u64, vertices: &[Vertex]) -> Result<()> {
        let index = self
            .content
            .meshes
            .iter()
            .position(|m| m.entity == entity)
            .ok_or_else(|| anyhow::anyhow!("Unknown mesh entity {entity}"))?;
        ensure!(
            vertices.len() == self.content.meshes[index].vertex_count,
            "Mesh update changes the vertex count"
        );
        ensure!(
            bytemuck::cast_slice::<Vertex, f32>(vertices)
                .iter()
                .all(|v| v.is_finite()),
            "Non-finite vertex"
        );
        let bytes: &[u8] = bytemuck::cast_slice(vertices);
        if !self.content.meshes[index].host {
            let buffer = self.gpu.create_buffer(
                bytes.len() as u64,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                true,
            )?;
            let old = self.content.meshes[index].vertices;
            self.content
                .geometry_inputs
                .retain(|(r, _)| *r != Resource::Buffer(old));
            self.gpu.retire(Resource::Buffer(old))?;
            self.content.meshes[index].vertices = buffer;
            self.content.meshes[index].host = true;
        }
        let mesh = &mut self.content.meshes[index];
        self.gpu.upload(mesh.vertices, bytes)?;
        mesh.centre = vertices
            .iter()
            .map(|v| Vec3::from_array(v.position))
            .sum::<Vec3>()
            / vertices.len() as f32;
        mesh.latest = Some(vertices.to_vec());
        self.content.revision += 1;
        Ok(())
    }

    /// Replace the local light list (at most 64 lights) for the next frame.
    pub fn set_lights(&mut self, lights: &[Light]) -> Result<()> {
        ensure!(
            lights.len() <= MAX_LIGHTS,
            "Tiled baseline supports at most 64 local lights"
        );
        ensure!(
            bytemuck::cast_slice::<Light, f32>(lights)
                .iter()
                .all(|v| v.is_finite()),
            "Non-finite light"
        );
        write_lights(
            &self.gpu,
            [self.content.light_buffer, self.content.compute_lights],
            lights,
        )?;
        self.content.lights = lights.to_vec();
        self.content.revision += 1;
        Ok(())
    }

    /// Replace the directional light: direction toward the light and linear RGB lux.
    pub fn set_sun(&mut self, direction: Vec3, lux: Vec3) -> Result<()> {
        ensure!(
            direction.is_finite() && direction.length_squared() > 0.001 && lux.is_finite(),
            "Invalid directional light"
        );
        self.content.sun_direction = direction.normalize();
        self.content.sun_lux = lux;
        self.content.revision += 1;
        Ok(())
    }

    /// Content hash for capture provenance, including dynamic updates since upload.
    pub(super) fn scene_hash(&self) -> String {
        if self.content.revision == 0 {
            return self.content.base_hash.clone();
        }
        let mut hash = Sha256::new();
        hash.update(self.content.base_hash.as_bytes());
        for mesh in &self.content.meshes {
            if let Some(latest) = &mesh.latest {
                hash.update(mesh.entity.to_le_bytes());
                hash.update(bytemuck::cast_slice(latest));
            }
        }
        hash.update(bytemuck::cast_slice(&self.content.lights));
        hash.update(bytemuck::bytes_of(&self.content.sun_direction));
        hash.update(bytemuck::bytes_of(&self.content.sun_lux));
        format!("{:x}", hash.finalize())
    }
}

/// Writes `lights` (or one zeroed placeholder) into both host-visible light lists.
fn write_lights(gpu: &VulkanDevice, buffers: [Buffer; 2], lights: &[Light]) -> Result<()> {
    let mut padded = lights.to_vec();
    if padded.is_empty() {
        padded.push(Light::zeroed());
    }
    for buffer in buffers {
        gpu.upload(buffer, bytemuck::cast_slice(&padded))?;
    }
    Ok(())
}

/// Uploaded materials, meshes and light lists (everything `update_geometry` swaps).
pub(super) struct Geometry {
    pub meshes: Vec<Mesh>,
    pub inputs: Vec<(Resource, Access)>,
    pub owned: Vec<Resource>,
    pub material_index: u32,
    pub lights: Vec<Light>,
    pub light_buffer: Buffer,
    pub light_index: u32,
    pub compute_lights: Buffer,
    pub compute_light_index: u32,
    pub entity_names: Vec<(u64, String)>,
}

/// Validates and uploads materials, meshes and host-visible light lists. Material
/// texture indices address `texture_indices`. Partial uploads are retired on error.
pub(super) fn upload_geometry(
    gpu: &mut VulkanDevice,
    materials: Vec<crate::scene::Material>,
    meshes: Vec<crate::scene::Mesh>,
    lights: Vec<Light>,
    texture_indices: &[u32],
) -> Result<Geometry> {
    ensure!(
        !materials.is_empty() && !meshes.is_empty(),
        "Scene has no geometry/materials"
    );
    ensure!(
        lights.len() <= MAX_LIGHTS,
        "Tiled baseline supports at most 64 local lights"
    );
    ensure!(meshes.len() < 16_777_216, "Float entity channel exhausted");
    ensure!(
        bytemuck::cast_slice::<Light, f32>(&lights)
            .iter()
            .all(|v| v.is_finite()),
        "Non-finite light"
    );
    ensure!(
        materials.iter().all(|m| m
            .base_color
            .iter()
            .chain(&m.emissive_roughness)
            .chain(&m.surface)
            .all(|v| v.is_finite())),
        "Non-finite material"
    );
    let mut seen = std::collections::BTreeSet::new();
    for mesh in &meshes {
        ensure!(seen.insert(mesh.entity), "Duplicate entity ID");
        ensure!(
            mesh.material < materials.len()
                && !mesh.vertices.is_empty()
                && !mesh.indices.is_empty()
                && mesh.indices.len() % 3 == 0,
            "Invalid mesh"
        );
        ensure!(
            mesh.indices
                .iter()
                .all(|i| (*i as usize) < mesh.vertices.len()),
            "Mesh index out of range"
        );
        ensure!(
            bytemuck::cast_slice::<Vertex, f32>(&mesh.vertices)
                .iter()
                .all(|v| v.is_finite()),
            "Non-finite vertex"
        );
    }
    let mut materials = materials;
    for material in &mut materials {
        for index in &mut material.textures {
            *index = *texture_indices
                .get(*index as usize)
                .ok_or_else(|| anyhow::anyhow!("Material texture out of bounds"))?;
        }
    }
    let mut created = Vec::new();
    let result = (|| -> Result<Geometry> {
        let material_buffer = upload_buffer(
            gpu,
            bytemuck::cast_slice(&materials),
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        created.push(Resource::Buffer(material_buffer));
        let material_index = gpu.storage_buffer(material_buffer)?.index;
        let mut inputs = vec![(Resource::Buffer(material_buffer), Access::ShaderRead)];
        let mut uploaded = Vec::new();
        let mut entity_names = Vec::new();
        for mesh in meshes {
            let centre = mesh
                .vertices
                .iter()
                .map(|v| Vec3::from_array(v.position))
                .sum::<Vec3>()
                / mesh.vertices.len() as f32;
            let vertices = upload_buffer(
                gpu,
                bytemuck::cast_slice(&mesh.vertices),
                vk::BufferUsageFlags::VERTEX_BUFFER,
            )?;
            created.push(Resource::Buffer(vertices));
            let indices = upload_buffer(
                gpu,
                bytemuck::cast_slice(&mesh.indices),
                vk::BufferUsageFlags::INDEX_BUFFER,
            )?;
            created.push(Resource::Buffer(indices));
            inputs.extend([
                (Resource::Buffer(vertices), Access::Vertex),
                (Resource::Buffer(indices), Access::Index),
            ]);
            entity_names.push((mesh.entity, mesh.name));
            uploaded.push(Mesh {
                entity: mesh.entity,
                vertices,
                indices,
                count: u32::try_from(mesh.indices.len())?,
                vertex_count: mesh.vertices.len(),
                material: mesh.material as u32,
                transparent: materials[mesh.material].flags[0] == 2,
                centre,
                host: false,
                latest: None,
            });
        }
        // Host-visible at full capacity so `set_lights` rewrites them in place;
        // contents match a static upload exactly.
        let bytes = (MAX_LIGHTS * std::mem::size_of::<Light>()) as u64;
        let light_buffer = gpu.create_buffer(bytes, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        created.push(Resource::Buffer(light_buffer));
        let compute_lights =
            gpu.create_buffer(bytes, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        created.push(Resource::Buffer(compute_lights));
        write_lights(gpu, [light_buffer, compute_lights], &lights)?;
        let light_index = gpu.storage_buffer(light_buffer)?.index;
        let compute_light_index = gpu.storage_buffer(compute_lights)?.index;
        Ok(Geometry {
            meshes: uploaded,
            inputs,
            owned: vec![
                Resource::Buffer(material_buffer),
                Resource::Buffer(light_buffer),
                Resource::Buffer(compute_lights),
            ],
            material_index,
            lights,
            light_buffer,
            light_index,
            compute_lights,
            compute_light_index,
            entity_names,
        })
    })();
    if result.is_err() {
        for resource in created {
            gpu.retire(resource)?;
        }
        gpu.collect()?;
    }
    result
}

fn upload_content(
    gpu: &mut VulkanDevice,
    scene: Scene,
    artifacts: &Path,
    shader_metadata: &crate::capture::ShaderMetadata,
    compute_frame: Buffer,
) -> Result<Content> {
    ensure!(
        scene.sun_direction.is_finite()
            && scene.sun_direction.length_squared() > 0.001
            && scene.sun_lux.is_finite(),
        "Invalid directional light"
    );
    for texture in &scene.textures {
        ensure!(
            texture.rgba.len() == texture.size[0] as usize * texture.size[1] as usize * 4,
            "Invalid texture pixels"
        );
    }
    ensure!(
        scene.environment.len()
            == scene.environment_size[0] as usize * scene.environment_size[1] as usize,
        "Invalid HDR environment"
    );
    ensure!(
        scene
            .environment
            .iter()
            .flatten()
            .all(|x| x.is_finite() && *x >= 0.),
        "HDR radiance must be finite and nonnegative"
    );
    let base_hash = super::hash::scene_hash(&scene);
    let static_hash = super::hash::static_hash(&scene);
    let mut static_owned = Vec::new();
    let mut static_inputs = Vec::new();
    let mut texture_indices = Vec::new();
    for texture in &scene.textures {
        let format = if texture.srgb {
            vk::Format::R8G8B8A8_SRGB
        } else {
            vk::Format::R8G8B8A8_UNORM
        };
        let image = upload_image(gpu, texture.size, format, &texture.rgba)?;
        static_owned.push(Resource::Image(image));
        texture_indices.push(gpu.sampled_image(image, false)?.index);
        static_inputs.push((Resource::Image(image), Access::ShaderRead));
    }
    let geometry = match upload_geometry(
        gpu,
        scene.materials,
        scene.meshes,
        scene.lights,
        &texture_indices,
    ) {
        Ok(geometry) => geometry,
        Err(error) => {
            for resource in static_owned {
                gpu.retire(resource)?;
            }
            gpu.collect()?;
            return Err(error);
        }
    };
    let environment_image = upload_image(
        gpu,
        scene.environment_size,
        vk::Format::R32G32B32A32_SFLOAT,
        bytemuck::cast_slice(&scene.environment),
    )?;
    let environment = gpu.sampled_image(environment_image, false)?.index;
    static_inputs.push((Resource::Image(environment_image), Access::ShaderRead));
    let usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE;
    let diffuse = gpu.create_image(32, 16, vk::Format::R32G32B32A32_SFLOAT, usage)?;
    let specular = gpu.create_image(256, 640, vk::Format::R32G32B32A32_SFLOAT, usage)?;
    static_owned.extend([
        Resource::Image(environment_image),
        Resource::Image(diffuse),
        Resource::Image(specular),
    ]);
    static_inputs.extend([
        (Resource::Image(diffuse), Access::ShaderRead),
        (Resource::Image(specular), Access::ShaderRead),
    ]);
    let irradiance = gpu.sampled_image(diffuse, false)?.index;
    let prefilter = gpu.sampled_image(specular, false)?.index;
    let diffuse_storage = gpu.storage_image(diffuse)?.index;
    let specular_storage = gpu.storage_image(specular)?.index;
    let compute_frame_index = gpu.storage_buffer(compute_frame)?.index;
    let mut ibl_frame = frame::Frame::zeroed();
    ibl_frame.extra[3] = environment;
    gpu.upload(compute_frame, bytemuck::bytes_of(&ibl_frame))?;
    let mut graph = Graph::default();
    let env = graph.import(
        Resource::Image(environment_image),
        State {
            access: Access::ShaderRead,
            queue: Queue::Graphics,
        },
    );
    let ir = graph.import(
        Resource::Image(diffuse),
        State {
            access: Access::Undefined,
            queue: Queue::Compute,
        },
    );
    let sp = graph.import(
        Resource::Image(specular),
        State {
            access: Access::Undefined,
            queue: Queue::Compute,
        },
    );
    let ibl_parameters = graph.import(
        Resource::Buffer(compute_frame),
        State {
            access: Access::HostWrite,
            queue: Queue::Compute,
        },
    );
    graph.pass(
        "EnvironmentRelease",
        Queue::Graphics,
        [(env, Access::ShaderRead)],
    );
    graph.pass(
        "IBL.Irradiance",
        Queue::Compute,
        [
            (env, Access::ShaderRead),
            (ir, Access::Storage),
            (ibl_parameters, Access::ShaderRead),
        ],
    );
    graph.pass(
        "IBL.SpecularPrefilter",
        Queue::Compute,
        [
            (env, Access::ShaderRead),
            (sp, Access::Storage),
            (ibl_parameters, Access::ShaderRead),
        ],
    );
    graph.pass(
        "IBL.Ready",
        Queue::Graphics,
        [
            (env, Access::ShaderRead),
            (ir, Access::ShaderRead),
            (sp, Access::ShaderRead),
        ],
    );
    let load = |gpu: &mut VulkanDevice, name: &str| -> Result<_> {
        let cooked: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
        ensure!(
            cooked.key == shader_metadata.content_key,
            "IBL and raster cooks must share one source cohort"
        );
        Ok(gpu.create_pipeline(&cooked, artifacts)?)
    };
    let irradiance_pipeline = load(gpu, "irradiance")?;
    let specular_pipeline = load(gpu, "prefilter")?;
    let compiled = graph.compile()?;
    let mut points = Vec::new();
    for (index, pass) in compiled.passes.iter().enumerate() {
        let mut recorder = gpu.begin(pass.queue);
        recorder.push(Command::BeginPass(pass.name.clone()));
        recorder.barriers(pass.before.iter().copied());
        if pass.id.0 == 1 || pass.id.0 == 2 {
            let (pipeline, storage, groups) = if pass.id.0 == 1 {
                (irradiance_pipeline, diffuse_storage, [4, 2, 1])
            } else {
                (specular_pipeline, specular_storage, [32, 80, 1])
            };
            recorder.push(Command::Pipeline(pipeline));
            recorder.push(Command::Constants(RootConstants {
                words: [compute_frame_index, storage, 0, 0, 0, 0, 0, 0],
            }));
            recorder.push(Command::Dispatch(groups));
        }
        recorder.barriers(pass.after.iter().copied());
        recorder.push(Command::EndPass);
        let waits = pass.waits.iter().map(|i| points[*i]).collect::<Vec<_>>();
        points.push(gpu.submit(recorder.finish(), &waits)?);
        ensure!(
            points.len() == index + 1,
            "Submission bookkeeping invariant"
        );
    }
    for point in points {
        gpu.resolve(point)?;
    }
    gpu.assert_clean()?;
    Ok(Content {
        meshes: geometry.meshes,
        geometry_inputs: geometry.inputs,
        static_inputs,
        geometry_owned: geometry.owned,
        static_owned,
        ibl_pipelines: [irradiance_pipeline, specular_pipeline],
        texture_indices,
        static_hash,
        material_index: geometry.material_index,
        lights: geometry.lights,
        light_buffer: geometry.light_buffer,
        light_index: geometry.light_index,
        compute_lights: geometry.compute_lights,
        compute_light_index: geometry.compute_light_index,
        environment,
        irradiance,
        prefilter,
        sun_direction: scene.sun_direction.normalize(),
        sun_lux: scene.sun_lux,
        base_hash,
        revision: 0,
        entity_names: geometry.entity_names,
    })
}
