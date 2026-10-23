use super::*;
use ara_gpu::{
    AccelerationStructure, Barrier,
    acceleration::{AccelerationDesc, AccelerationInstance},
};
/// Optional exact-source ray-query shadow reference. It shares the rasterized
/// surface sample, isolating shadow representation error from camera visibility.
pub struct ShadowReference {
    vertices: Buffer,
    indices: Buffer,
    blas: AccelerationStructure,
    tlas: AccelerationStructure,
    tlas_slot: u32,
    world_slot: u32,
    normal_slot: u32,
    output: Image,
    output_slot: u32,
    pipeline: Pipeline,
    parameters: Buffer,
    parameter_slot: u32,
    state: Access,
}
impl ShadowReference {
    /// Builds one shared full-resolution BLAS and an instanced TLAS off-thread.
    pub fn new(
        gpu: &mut VulkanDevice,
        source: &ara_geo::SourceMesh,
        instances: &[Instance],
        geometry: &GeometryView,
        cooked: &Path,
    ) -> Result<Self> {
        ensure!(
            gpu.context().info.ray_tracing,
            "RT shadow reference unavailable"
        );
        ensure!(
            !source.vertices.is_empty()
                && !source.indices.is_empty()
                && source.indices.len().is_multiple_of(3)
                && source
                    .indices
                    .iter()
                    .all(|&i| (i as usize) < source.vertices.len()),
            "invalid RT source geometry"
        );
        let usage = vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
        let vertices =
            super::super::init::upload(gpu, bytemuck::cast_slice(&source.vertices), usage)?;
        let indices =
            super::super::init::upload(gpu, bytemuck::cast_slice(&source.indices), usage)?;
        let blas = gpu.create_acceleration_structure(&AccelerationDesc::Triangles {
            vertices,
            vertex_count: u32::try_from(source.vertices.len())?,
            vertex_stride: size_of::<ara_geo::Vertex>() as u64,
            indices,
            index_count: u32::try_from(source.indices.len())?,
        })?;
        let mut r = gpu.begin(Queue::Graphics);
        for b in [vertices, indices] {
            r.barriers([Barrier {
                resource: Resource::Buffer(b),
                before: Access::ShaderRead,
                after: Access::AsInput,
                handoff: None,
            }]);
        }
        r.push(Command::BuildAccelerationStructure(blas));
        r.barriers([Barrier {
            resource: Resource::AccelerationStructure(blas),
            before: Access::AsBuild,
            after: Access::AsInput,
            handoff: None,
        }]);
        let p = gpu.submit(r.finish(), &[])?;
        gpu.resolve(p)?;
        let instances = instances
            .iter()
            .enumerate()
            .map(|(index, i)| {
                let [x, y, z, s] = i.position_scale;
                AccelerationInstance {
                    geometry: blas,
                    transform: [s, 0.0, 0.0, x, 0.0, s, 0.0, y, 0.0, 0.0, s, z],
                    custom_index: index as u32,
                    mask: 255,
                }
            })
            .collect();
        let tlas = gpu.create_acceleration_structure(&AccelerationDesc::Instances(instances))?;
        let mut r = gpu.begin(Queue::Graphics);
        r.push(Command::BuildAccelerationStructure(tlas));
        r.barriers([Barrier {
            resource: Resource::AccelerationStructure(tlas),
            before: Access::AsBuild,
            after: Access::AsRead,
            handoff: None,
        }]);
        let p = gpu.submit(r.finish(), &[])?;
        gpu.resolve(p)?;
        let tlas_slot = gpu.bind_acceleration_structure(tlas)?.index;
        let output = gpu.create_image(
            geometry.size[0],
            geometry.size[1],
            vk::Format::R32G32B32A32_SFLOAT,
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let output_slot = gpu.storage_image(output)?.index;
        let world_slot = gpu.sampled_image(geometry.images[2], false)?.index;
        let normal_slot = gpu.sampled_image(geometry.images[1], false)?.index;
        let parameters = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        let parameter_slot = gpu.storage_buffer(parameters)?.index;
        let manifest: CookedPipeline =
            serde_json::from_slice(&std::fs::read(cooked.join("shadow_reference.json"))?)?;
        let pipeline = gpu.create_pipeline(&manifest, cooked)?;
        Ok(Self {
            vertices,
            indices,
            blas,
            tlas,
            tlas_slot,
            world_slot,
            normal_slot,
            output,
            output_slot,
            pipeline,
            parameters,
            parameter_slot,
            state: Access::Undefined,
        })
    }
    /// Executes one directional hard-shadow query per covered surface sample.
    /// This diagnostic reference resolves synchronously and is timed separately.
    pub fn render(
        &mut self,
        gpu: &mut VulkanDevice,
        geometry: &GeometryView,
        toward_light: Vec3,
    ) -> Result<Vec<ara_gpu::commands::PassTiming>> {
        ensure!(
            toward_light.is_finite() && toward_light.length_squared() > 1e-12,
            "invalid RT sun direction"
        );
        gpu.upload(
            self.parameters,
            bytemuck::cast_slice(&toward_light.normalize().extend(0.0).to_array()),
        )?;
        let mut g = Graph::default();
        let out = g.import(
            Resource::Image(self.output),
            State {
                access: self.state,
                queue: Queue::Graphics,
            },
        );
        let mut uses = vec![(out, Access::Storage)];
        for i in [geometry.images[1], geometry.images[2]] {
            uses.push((
                g.import(
                    Resource::Image(i),
                    State {
                        access: Access::ShaderRead,
                        queue: Queue::Graphics,
                    },
                ),
                Access::ShaderRead,
            ));
        }
        uses.push((
            g.import(
                Resource::AccelerationStructure(self.tlas),
                State {
                    access: Access::AsRead,
                    queue: Queue::Graphics,
                },
            ),
            Access::AsRead,
        ));
        uses.push((
            g.import(
                Resource::Buffer(self.parameters),
                State {
                    access: Access::HostWrite,
                    queue: Queue::Graphics,
                },
            ),
            Access::ShaderRead,
        ));
        g.pass("Shadow.RT.Reference", Queue::Graphics, uses);
        g.pass(
            "Shadow.RT.Publish",
            Queue::Graphics,
            [(out, Access::ShaderRead)],
        );
        let compiled = g.compile()?;
        let mut r = gpu.begin(Queue::Graphics);
        for p in compiled.passes {
            r.barriers(p.before);
            if p.name == "Shadow.RT.Reference" {
                r.push(Command::BeginPass(p.name));
                r.push(Command::Pipeline(self.pipeline));
                r.push(Command::Constants(RootConstants {
                    words: [
                        self.tlas_slot,
                        self.world_slot,
                        self.normal_slot,
                        self.output_slot,
                        geometry.size[0],
                        geometry.size[1],
                        self.parameter_slot,
                        0,
                    ],
                }));
                r.push(Command::Dispatch([
                    geometry.size[0].div_ceil(8),
                    geometry.size[1].div_ceil(8),
                    1,
                ]));
                r.push(Command::EndPass);
            }
            r.barriers(p.after);
        }
        let p = gpu.submit(r.finish(), &[])?;
        let timings = gpu.resolve(p)?;
        self.state = Access::ShaderRead;
        Ok(timings)
    }
    /// Reads completed shadow visibility for numerical/image comparisons.
    pub fn samples(
        &self,
        gpu: &mut VulkanDevice,
        geometry: &GeometryView,
    ) -> Result<Vec<[f32; 4]>> {
        Ok(geometry
            .read_image(gpu, self.output, 16)?
            .as_chunks::<16>()
            .0
            .iter()
            .map(|b| bytemuck::pod_read_unaligned(b))
            .collect())
    }
    /// Retires TLAS before its BLAS and source buffers.
    pub fn retire(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::AccelerationStructure(self.tlas))?;
        gpu.retire(Resource::AccelerationStructure(self.blas))?;
        for b in [self.vertices, self.indices, self.parameters] {
            gpu.retire(Resource::Buffer(b))?;
        }
        gpu.retire(Resource::Image(self.output))?;
        gpu.retire_pipeline(self.pipeline)?;
        gpu.collect()?;
        Ok(())
    }
}
