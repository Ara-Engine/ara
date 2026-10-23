use super::transfer::{upload_buffer, upload_buffer_on, upload_image};
use super::*;
use crate::graph::{Graph, State};
use crate::scene::Vertex;
use ara_gpu::{RootConstants, vulkan::Command};
use bytemuck::Zeroable;
use sha2::{Digest, Sha256};

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
        ensure!(
            !scene.materials.is_empty() && !scene.meshes.is_empty(),
            "Scene has no geometry/materials"
        );
        ensure!(
            scene.lights.len() <= 64,
            "Tiled baseline supports at most 64 local lights"
        );
        ensure!(
            scene.sun_direction.is_finite()
                && scene.sun_direction.length_squared() > 0.001
                && scene.sun_lux.is_finite(),
            "Invalid directional light"
        );
        let mut hash = Sha256::new();
        let mut graphics_inputs = Vec::new();
        let mut texture_indices = Vec::new();
        for texture in &scene.textures {
            ensure!(
                texture.rgba.len() == texture.size[0] as usize * texture.size[1] as usize * 4,
                "Invalid texture pixels"
            );
            hash.update(texture.size[0].to_le_bytes());
            hash.update(texture.size[1].to_le_bytes());
            hash.update([u8::from(texture.srgb)]);
            hash.update(&texture.rgba);
            let format = if texture.srgb {
                vk::Format::R8G8B8A8_SRGB
            } else {
                vk::Format::R8G8B8A8_UNORM
            };
            let image = upload_image(&mut gpu, texture.size, format, &texture.rgba)?;
            texture_indices.push(gpu.sampled_image(image, false)?.index);
            graphics_inputs.push((Resource::Image(image), Access::ShaderRead));
        }
        let mut materials = scene.materials;
        hash.update(bytemuck::cast_slice(&materials));
        for material in &mut materials {
            for index in &mut material.textures {
                *index = *texture_indices
                    .get(*index as usize)
                    .ok_or_else(|| anyhow::anyhow!("Material texture out of bounds"))?;
            }
        }
        let material_buffer = upload_buffer(
            &mut gpu,
            bytemuck::cast_slice(&materials),
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let material_index = gpu.storage_buffer(material_buffer)?.index;
        graphics_inputs.push((Resource::Buffer(material_buffer), Access::ShaderRead));
        let mut meshes = Vec::new();
        let mut entity_names = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for mesh in scene.meshes {
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
                mesh.vertices
                    .iter()
                    .all(
                        |v| bytemuck::cast_slice::<Vertex, f32>(std::slice::from_ref(v))
                            .iter()
                            .all(|v| v.is_finite())
                    ),
                "Non-finite vertex"
            );
            hash.update(mesh.entity.to_le_bytes());
            hash.update((mesh.name.len() as u64).to_le_bytes());
            hash.update(mesh.name.as_bytes());
            hash.update(bytemuck::cast_slice(&mesh.vertices));
            hash.update(bytemuck::cast_slice(&mesh.indices));
            hash.update((mesh.material as u64).to_le_bytes());
            let centre = mesh
                .vertices
                .iter()
                .map(|v| Vec3::from_array(v.position))
                .sum::<Vec3>()
                / mesh.vertices.len() as f32;
            let vertices = upload_buffer(
                &mut gpu,
                bytemuck::cast_slice(&mesh.vertices),
                vk::BufferUsageFlags::VERTEX_BUFFER,
            )?;
            let indices = upload_buffer(
                &mut gpu,
                bytemuck::cast_slice(&mesh.indices),
                vk::BufferUsageFlags::INDEX_BUFFER,
            )?;
            graphics_inputs.extend([
                (Resource::Buffer(vertices), Access::Vertex),
                (Resource::Buffer(indices), Access::Index),
            ]);
            entity_names.push((mesh.entity, mesh.name.clone()));
            meshes.push(Mesh {
                vertices,
                indices,
                count: u32::try_from(mesh.indices.len())?,
                material: mesh.material as u32,
                transparent: materials[mesh.material].flags[0] == 2,
                centre,
            });
        }
        ensure!(meshes.len() < 16_777_216, "Float entity channel exhausted");
        hash.update(bytemuck::cast_slice(&scene.lights));
        hash.update(bytemuck::bytes_of(&scene.sun_direction));
        hash.update(bytemuck::bytes_of(&scene.sun_lux));
        let lights = if scene.lights.is_empty() {
            vec![crate::scene::Light::zeroed()]
        } else {
            scene.lights
        };
        let light_count = if lights.len() == 1 && lights[0].color_lumens[3] == 0. {
            0
        } else {
            lights.len() as u32
        };
        let light_buffer = upload_buffer(
            &mut gpu,
            bytemuck::cast_slice(&lights),
            vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let light_index = gpu.storage_buffer(light_buffer)?.index;
        graphics_inputs.push((Resource::Buffer(light_buffer), Access::ShaderRead));
        let compute_lights = upload_buffer_on(
            &mut gpu,
            bytemuck::cast_slice(&lights),
            vk::BufferUsageFlags::STORAGE_BUFFER,
            Queue::Compute,
        )?;
        let compute_light_index = gpu.storage_buffer(compute_lights)?.index;
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
        hash.update(scene.environment_size[0].to_le_bytes());
        hash.update(scene.environment_size[1].to_le_bytes());
        hash.update(bytemuck::cast_slice(&scene.environment));
        let environment_image = upload_image(
            &mut gpu,
            scene.environment_size,
            vk::Format::R32G32B32A32_SFLOAT,
            bytemuck::cast_slice(&scene.environment),
        )?;
        let environment = gpu.sampled_image(environment_image, false)?.index;
        graphics_inputs.push((Resource::Image(environment_image), Access::ShaderRead));
        let usage = vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE;
        let diffuse = gpu.create_image(32, 16, vk::Format::R32G32B32A32_SFLOAT, usage)?;
        let specular = gpu.create_image(256, 640, vk::Format::R32G32B32A32_SFLOAT, usage)?;
        graphics_inputs.extend([
            (Resource::Image(diffuse), Access::ShaderRead),
            (Resource::Image(specular), Access::ShaderRead),
        ]);
        let irradiance = gpu.sampled_image(diffuse, false)?.index;
        let prefilter = gpu.sampled_image(specular, false)?.index;
        let diffuse_storage = gpu.storage_image(diffuse)?.index;
        let specular_storage = gpu.storage_image(specular)?.index;
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
        let irradiance_pipeline = load(&mut gpu, "irradiance")?;
        let specular_pipeline = load(&mut gpu, "prefilter")?;
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
            for timing in gpu.resolve(point)? {
                println!("{}: {:.3} ms", timing.pass, timing.milliseconds);
            }
        }
        gpu.assert_clean()?;
        Ok(Self {
            gpu,
            size,
            targets,
            pipelines,
            meshes,
            graphics_inputs,
            compute_inputs: vec![(Resource::Buffer(compute_lights), Access::ShaderRead)],
            material_index,
            frame,
            frame_index,
            compute_frame,
            compute_frame_index,
            light_index,
            compute_light_index,
            light_count,
            environment,
            irradiance,
            prefilter,
            sun_direction: scene.sun_direction.normalize(),
            sun_lux: scene.sun_lux,
            previous_vp: Mat4::IDENTITY,
            frame_number: 0,
            scene_hash: format!("{:x}", hash.finalize()),
            shader_metadata,
            entity_names,
            last_timings: Vec::new(),
        })
    }
}
