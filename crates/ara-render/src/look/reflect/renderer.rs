//! Minimal native integration for the opt-in software secondary path.
use super::*;
use crate::{
    graph::{Graph, PassId, ResourceId},
    look::{
        reflect::{ReflectionCounters, Scene as TraceSource, SoftwareReflections},
        trace::{BlasCache, TraceGpu},
    },
};
use anyhow::Context;
use ara_gpu::{
    Barrier, RootConstants,
    commands::{Command, Recorder},
};

pub(super) struct Reflections {
    settings: SoftwareReflections,
    source: TraceSource,
    trace: TraceGpu,
    cache: BlasCache,
    entries: Buffer,
    entry_index: u32,
    entry_bytes: usize,
    entry_generation: Option<Buffer>,
    counters: Buffer,
    counter_index: u32,
    counter_state: Access,
    image: Option<(Image, u32, u32, [u32; 2], State)>,
    psos: [Pipeline; 6],
    ids: Vec<ResourceId>,
    output: Option<ResourceId>,
}

impl Renderer {
    /// Enable the experimental native-resolution software reflection/glass path.
    /// `None` restores the default. Call at a completed frame boundary with the
    /// published scene. Subsequent native scene updates synchronize it automatically.
    /// Requires native resolution; the opt-in path also bypasses temporal resolve.
    pub fn set_software_reflections(
        &mut self,
        settings: Option<SoftwareReflections>,
        scene: &crate::instanced::Scene,
    ) -> Result<()> {
        ensure!(
            !self.gpu.deferring_completion(),
            "software reflections require a completed frame boundary"
        );
        if let Some(settings) = settings {
            settings.validate()?;
            ensure!(
                self.look.quality.ray_queries.is_none(),
                "software and hardware secondary paths are exclusive"
            );
            ensure!(
                self.patch_transport.is_none(),
                "software secondary transport currently reads today's probe GI"
            );
            scene.validated()?;
            if let Some(ref mut reflection) = self.software_reflections {
                reflection.settings = settings;
            } else {
                let native = self
                    .native
                    .as_ref()
                    .context("software reflections need native content")?;
                let reflection =
                    Reflections::new(&mut self.gpu, &self.artifacts, settings, scene, native)?;
                self.software_reflections = Some(reflection);
            }
            self.update_software_reflections(scene)?;
        } else if let Some(reflection) = self.software_reflections.take() {
            reflection.destroy(&mut self.gpu)?;
        }
        self.reset_temporal_history();
        Ok(())
    }

    pub(super) fn update_software_reflections(
        &mut self,
        scene: &crate::instanced::Scene,
    ) -> Result<()> {
        if let Some(ref mut reflection) = self.software_reflections {
            let native = self
                .native
                .as_ref()
                .context("software reflection content disappeared")?;
            if reflection.source.update(scene, &reflection.cache)? {
                // A fresh CPU scene may repeat the old version numbers. Publish
                // a fresh GPU scene rather than interpreting them as unchanged.
                let next = TraceGpu::new(&mut self.gpu, &reflection.source.trace)?;
                std::mem::replace(&mut reflection.trace, next).destroy(&mut self.gpu)?;
            } else {
                reflection
                    .trace
                    .update(&mut self.gpu, &reflection.source.trace)?;
            }
            reflection.update_entries(&mut self.gpu, native)?;
        }
        Ok(())
    }

    /// Read completed native-pixel work counters. This explicit diagnostic read
    /// waits for its short barrier submission; it is not part of live rendering.
    pub fn software_reflection_counters(&mut self) -> Result<Option<ReflectionCounters>> {
        let Some(ref mut reflection) = self.software_reflections else {
            return Ok(None);
        };
        ensure!(
            !self.gpu.deferring_completion(),
            "finish rendering before reading reflection counters"
        );
        let mut recorder = self.gpu.begin(Queue::Graphics);
        recorder.barriers([Barrier {
            resource: Resource::Buffer(reflection.counters),
            before: reflection.counter_state,
            after: Access::HostRead,
            handoff: None,
        }]);
        let point = self.gpu.submit(recorder.finish(), &[])?;
        self.gpu.resolve(point)?;
        reflection.counter_state = Access::HostRead;
        let bytes = self.gpu.read(reflection.counters)?;
        let w: &[u32] = bytemuck::cast_slice(&bytes);
        Ok(Some(ReflectionCounters {
            mirror_pixels: w[0],
            rough_pixels: w[1],
            rays: w[2],
            visibility_rays: w[3],
            glass_interfaces: w[4],
            exhausted: w[5],
            invalid_media: w[6],
            prefiltered_pixels: w[7],
        }))
    }
}

impl Reflections {
    fn new(
        gpu: &mut GpuDevice,
        artifacts: &Path,
        settings: SoftwareReflections,
        scene: &crate::instanced::Scene,
        native: &native::Native,
    ) -> Result<Self> {
        let cache = BlasCache::new(
            std::env::var_os("ARA_REFLECT_CACHE")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../target/ara-render/reflect-bvh")
                }),
        );
        let mut source = TraceSource::new();
        source.update(scene, &cache)?;
        let trace = TraceGpu::new(gpu, &source.trace)?;
        let mut psos = Vec::new();
        for name in [
            "reflect_clear",
            "reflect_mirror",
            "reflect_rough",
            "reflect_prefiltered",
            "reflect_lighting",
            "glass_ordered",
        ] {
            let recipe: ara_gpu::cooked::CookedPipeline =
                serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
            psos.push(gpu.create_pipeline(&recipe, artifacts)?);
        }
        let entry_bytes = (native.ray_tables().instances * 4).max(16);
        let entries = gpu.create_buffer(
            entry_bytes as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let counters = gpu.create_buffer(32, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        gpu.upload(counters, &[0; 32])?;
        let entry_index = gpu.storage_buffer(entries)?.index;
        let counter_index = gpu.storage_buffer(counters)?.index;
        let mut result = Self {
            settings,
            source,
            trace,
            cache,
            entries,
            entry_index,
            entry_bytes,
            entry_generation: None,
            counters,
            counter_index,
            counter_state: Access::HostWrite,
            image: None,
            psos: psos
                .try_into()
                .map_err(|_| anyhow::anyhow!("software reflection pipeline count"))?,
            ids: Vec::new(),
            output: None,
        };
        result.update_entries(gpu, native)?;
        Ok(result)
    }

    fn update_entries(&mut self, gpu: &mut GpuDevice, native: &native::Native) -> Result<()> {
        let generation = native.ray_tables().rows;
        if self.entry_generation == Some(generation) {
            return Ok(());
        }
        let values: Vec<u32> = (0..native.ray_tables().instances)
            .map(|i| native.ray_instance(i).1)
            .collect();
        if values.len() * 4 > self.entry_bytes {
            self.entry_bytes = (values.len() * 4).next_power_of_two();
            let next = gpu.create_buffer(
                self.entry_bytes as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )?;
            gpu.retire(Resource::Buffer(std::mem::replace(&mut self.entries, next)))?;
            self.entry_index = gpu.storage_buffer(next)?.index;
        }
        if !values.is_empty() {
            gpu.upload_queued_range(self.entries, 0, bytemuck::cast_slice(&values))?;
        }
        self.entry_generation = Some(generation);
        Ok(())
    }

    pub(super) fn prepare(&mut self, gpu: &mut GpuDevice, size: [u32; 2]) -> Result<()> {
        if self.image.as_ref().is_none_or(|i| i.3 != size) {
            let image = gpu.create_image(
                size[0],
                size[1],
                vk::Format::R32G32B32A32_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::STORAGE,
            )?;
            let sampled = gpu.sampled_image(image, false)?.index;
            let storage = gpu.storage_image(image)?.index;
            if let Some(old) = self.image.replace((
                image,
                sampled,
                storage,
                size,
                State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                },
            )) {
                gpu.retire(Resource::Image(old.0))?;
            }
        }
        Ok(())
    }

    pub(super) fn rows(
        &self,
        frame: &mut crate::look::frame::LookFrame,
        native: &native::Native,
        projection: f32,
        size: [u32; 2],
    ) {
        let tables = native.ray_tables();
        let s = self.settings;
        frame.rt_info = [
            [
                self.trace.record(),
                tables.rows_index,
                tables.geometry,
                tables.materials,
            ],
            [
                tables.textures,
                self.entry_index,
                self.image.as_ref().map_or(0, |i| i.1),
                self.counter_index,
            ],
        ];
        frame.rt_params = [
            [
                s.mirror_cut,
                s.distance,
                2. / (projection.abs() * size[1] as f32),
                s.rough_rays as f32,
            ],
            [0.0005, s.interfaces as f32, 1., s.rough_cut],
        ];
    }

    pub(super) fn declare(
        &mut self,
        graph: &mut Graph,
        reads: &[(ResourceId, Access)],
    ) -> Vec<(PassId, u32)> {
        self.ids.clear();
        for (i, buffer) in self.trace.buffers().into_iter().enumerate() {
            self.ids.push(graph.import(
                Resource::Buffer(buffer),
                State {
                    access: if i < 3 {
                        Access::ShaderRead
                    } else {
                        Access::HostWrite
                    },
                    queue: Queue::Graphics,
                },
            ));
        }
        self.ids.push(graph.import(
            Resource::Buffer(self.entries),
            State {
                access: Access::HostWrite,
                queue: Queue::Graphics,
            },
        ));
        let counter = graph.import(
            Resource::Buffer(self.counters),
            State {
                access: self.counter_state,
                queue: Queue::Graphics,
            },
        );
        self.ids.push(counter);
        let image = self.image.as_ref().expect("prepared reflection image");
        let out = graph.import(Resource::Image(image.0), image.4);
        self.output = Some(out);
        let mut passes = vec![(
            graph.pass(
                "Reflect.Clear",
                Queue::Graphics,
                reads.iter().copied().chain([(counter, Access::Storage)]),
            ),
            0,
        )];
        for (stage, name) in [
            (1, "Reflect.Mirror"),
            (2, "Reflect.Rough"),
            (3, "Reflect.Prefiltered"),
        ] {
            passes.push((
                graph.pass(
                    name,
                    Queue::Graphics,
                    reads
                        .iter()
                        .copied()
                        .chain(self.reads())
                        .chain([(out, Access::Storage)]),
                ),
                stage,
            ));
        }
        self.counter_state = Access::Storage;
        passes
    }

    pub(super) fn reads(&self) -> impl Iterator<Item = (ResourceId, Access)> + '_ {
        self.ids.iter().enumerate().map(|(i, &id)| {
            (
                id,
                if i == 9 {
                    Access::Storage
                } else {
                    Access::ShaderRead
                },
            )
        })
    }
    pub(super) fn lighting_read(&self) -> Option<(ResourceId, Access)> {
        self.output.map(|id| (id, Access::ShaderRead))
    }
    pub(super) fn lighting(&self) -> Pipeline {
        self.psos[4]
    }
    pub(super) fn glass(&self) -> Pipeline {
        self.psos[5]
    }
    pub(super) fn record(&self, recorder: &mut Recorder, stage: u32, frame: u32, size: [u32; 2]) {
        recorder.push(Command::Pipeline(self.psos[stage as usize]));
        recorder.push(Command::Constants(RootConstants {
            words: [
                frame,
                self.image.as_ref().expect("prepared image").2,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        }));
        recorder.push(Command::Dispatch(if stage == 0 {
            [1, 1, 1]
        } else {
            [size[0].div_ceil(8), size[1].div_ceil(8), 1]
        }));
    }
    pub(super) fn finish(&mut self, states: &[State]) {
        if let (Some(image), Some(id)) = (&mut self.image, self.output) {
            image.4 = states[id.0];
        }
    }
    fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        self.trace.destroy(gpu)?;
        for b in [self.entries, self.counters] {
            gpu.retire(Resource::Buffer(b))?;
        }
        if let Some(image) = self.image {
            gpu.retire(Resource::Image(image.0))?;
        }
        for p in self.psos {
            gpu.retire_pipeline(p)?;
        }
        Ok(())
    }
}
