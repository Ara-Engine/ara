use super::*;
use ara_gpu::{Barrier, RootConstants, commands::Command};

pub(crate) struct GatherHarness {
    pub gpu: GpuDevice,
    pipeline: Pipeline,
    record: Buffer,
    record_index: u32,
    values: [Buffer; 2],
    indices: [u32; 2],
    readback: Buffer,
    tiles: u32,
    count: usize,
    slot: usize,
}
impl GatherHarness {
    pub fn new(links: &LinkStore, solver: &EventSolver) -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut gpu =
            GpuDevice::new_raster(&[], root.join("../../target/ara-render/transport-gather"))?;
        let artifacts = ara_gpu::cooked::resource_dir(
            env!("CARGO_PKG_NAME"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
        );
        let pipeline = load(&mut gpu, &artifacts, "transport_gather")?;
        let active: Vec<_> = (0..links.tiles.len() as u32).collect();
        let data = gather_bytes(links, solver, &active, 1);
        let record = gpu.create_buffer(
            data.len() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        gpu.upload(record, &data)?;
        let record_index = gpu.storage_buffer(record)?.index;
        let bytes = (solver.radiance.len() * 12).max(16) as u64;
        let usage = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC;
        let values = [
            gpu.create_buffer(bytes, usage, true)?,
            gpu.create_buffer(bytes, usage, true)?,
        ];
        let indices = [
            gpu.storage_buffer(values[0])?.index,
            gpu.storage_buffer(values[1])?.index,
        ];
        for buffer in values {
            gpu.upload(buffer, bytemuck::cast_slice(&solver.radiance))?;
        }
        let readback = gpu.create_buffer(bytes, vk::BufferUsageFlags::TRANSFER_DST, true)?;
        Ok(Self {
            gpu,
            pipeline,
            record,
            record_index,
            values,
            indices,
            readback,
            tiles: active.len() as u32,
            count: solver.radiance.len(),
            slot: 0,
        })
    }

    pub fn step(&mut self, first: bool) -> Result<f64> {
        let output = 1 - self.slot;
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.barriers([
            Barrier {
                resource: Resource::Buffer(self.record),
                before: if first {
                    Access::HostWrite
                } else {
                    Access::ShaderRead
                },
                after: Access::ShaderRead,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(self.values[self.slot]),
                before: if first {
                    Access::HostWrite
                } else {
                    Access::Storage
                },
                after: Access::ShaderRead,
                handoff: None,
            },
            Barrier {
                resource: Resource::Buffer(self.values[output]),
                before: if first {
                    Access::HostWrite
                } else {
                    Access::ShaderRead
                },
                after: Access::Storage,
                handoff: None,
            },
        ]);
        commands.push(Command::BeginPass("Transport.Gather".into()));
        commands.push(Command::Pipeline(self.pipeline));
        commands.push(Command::Constants(RootConstants {
            words: [
                self.record_index,
                self.indices[self.slot],
                self.indices[output],
                0,
                0,
                0,
                0,
                0,
            ],
        }));
        commands.push(Command::Dispatch([
            self.tiles.min(65_535),
            self.tiles.div_ceil(65_535),
            1,
        ]));
        commands.push(Command::EndPass);
        let point = self.gpu.submit(commands.finish(), &[])?;
        let timings = self.gpu.resolve(point)?;
        self.slot = output;
        Ok(timings
            .iter()
            .filter(|t| t.pass == "Transport.Gather")
            .map(|t| t.milliseconds)
            .sum())
    }

    pub fn read(&mut self) -> Result<Vec<Vec3>> {
        let mut commands = self.gpu.begin(Queue::Graphics);
        commands.barriers([Barrier {
            resource: Resource::Buffer(self.values[self.slot]),
            before: Access::Storage,
            after: Access::CopySource,
            handoff: None,
        }]);
        commands.push(Command::CopyBuffer {
            source: self.values[self.slot],
            destination: self.readback,
            size: (self.count * 12) as u64,
        });
        commands.barriers([Barrier {
            resource: Resource::Buffer(self.values[self.slot]),
            before: Access::CopySource,
            after: Access::Storage,
            handoff: None,
        }]);
        let point = self.gpu.submit(commands.finish(), &[])?;
        self.gpu.resolve(point)?;
        let bytes = self.gpu.read(self.readback)?;
        Ok(bytes[..self.count * 12]
            .as_chunks::<12>()
            .0
            .iter()
            .map(|b| {
                Vec3::from_array(std::array::from_fn(|i| {
                    f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().expect("float"))
                }))
            })
            .collect())
    }
}

#[test]
fn transport_gpu_gather_matches_cpu_and_converged_oracle() -> Result<()> {
    let links = super::super::tests::two_patches()?;
    let mut solver = EventSolver::new(&links)?;
    solver.set_forcing(0, Vec3::new(1., 0.2, 0.05))?;
    let expected = solver.exact(&links, 1e-10)?;
    let mut gpu = GatherHarness::new(&links, &solver)?;
    for iteration in 0..512 {
        gpu.step(iteration == 0)?;
    }
    let actual = gpu.read()?;
    for (a, b) in actual.iter().zip(expected) {
        assert!((*a - b).abs().max_element() < 2e-5, "{a:?} != {b:?}");
    }
    gpu.gpu.assert_clean()?;
    Ok(())
}

#[test]
#[ignore = "quiet-window GPU measurement; explicit B3 command only"]
fn transport_gpu_gather_timing() -> Result<()> {
    use crate::look::transport::{
        links::{Candidate, Visibility},
        patches::{PatchRegistry, Placement},
    };
    use ara_patch_charts::{Mesh, Settings, Triangle, cache};
    use std::sync::Arc;
    let mesh = Mesh {
        positions: &[[0., 0., 0.], [80., 0., 0.], [80., 60., 0.], [0., 60., 0.]],
        triangles: &[
            Triangle {
                vertices: [0, 1, 2],
                material: 0,
                albedo: [0.7; 3],
            },
            Triangle {
                vertices: [0, 2, 3],
                material: 0,
                albedo: [0.7; 3],
            },
        ],
    };
    let prepared = cache::prepare(mesh, Settings::default(), None)?;
    let mut registry = PatchRegistry::default();
    registry
        .publish(
            1,
            prepared.key,
            Arc::new(prepared.charts),
            0,
            Placement::default(),
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let patches = Arc::new(registry.snapshot().map_err(|e| anyhow::anyhow!("{e:?}"))?);
    let count = patches.len() as u32;
    let links = LinkStore::build(patches, vec![], count as usize * 48, |p| {
        Ok((0..48)
            .map(|ordinal| Candidate {
                ordinal,
                source: (p + ordinal * 127 + 1) % count,
                weight: 1. / 48.,
                visibility: Visibility::clear(),
                segment: None,
            })
            .collect())
    })?;
    let mut solver = EventSolver::new(&links)?;
    for p in 0..links.patches.len() as u32 {
        solver.set_forcing(p, Vec3::splat(0.5))?;
    }
    let mut gpu = GatherHarness::new(&links, &solver)?;
    let mut samples = Vec::new();
    for i in 0..180 {
        let ms = gpu.step(i == 0)?;
        if i >= 30 {
            samples.push(ms);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "transport_gather patches={} links={} samples={} p50_ms={:.6} p99_ms={:.6} max_ms={:.6}",
        links.patches.len(),
        links.len(),
        samples.len(),
        samples[samples.len() / 2],
        samples[(samples.len() * 99 / 100).min(samples.len() - 1)],
        samples[samples.len() - 1]
    );
    gpu.gpu.assert_clean()?;
    Ok(())
}
