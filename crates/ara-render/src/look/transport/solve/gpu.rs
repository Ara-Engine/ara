//! GPU packet gather and native-resolution readout resources. Uploads and graph
//! dependencies are explicit; disabled renderers allocate no transport resources.
use super::{EventSolver, PatchTransport};
use crate::{
    graph::{Graph, ResourceId, State},
    instanced::Scene,
    look::transport::links::LinkStore,
};
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, GpuDevice, Pipeline, Queue, Resource, vk};
use glam::Vec3;
use std::{collections::BTreeMap, path::Path};

pub(crate) struct ReadoutGpu {
    pub buffer: Buffer,
    pub record: u32,
    pub lit: Pipeline,
    pub diffuse: Pipeline,
    pub bytes: usize,
    access: Access,
}

fn load(gpu: &mut GpuDevice, artifacts: &Path, name: &str) -> Result<Pipeline> {
    let cooked: ara_gpu::cooked::CookedPipeline =
        serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
    Ok(gpu.create_pipeline(&cooked, artifacts)?)
}

impl ReadoutGpu {
    pub fn new(
        gpu: &mut GpuDevice,
        artifacts: &Path,
        transport: &PatchTransport,
        scene: &Scene,
    ) -> Result<Self> {
        let packed = readout_bytes(transport, scene, None);
        let buffer = gpu.create_buffer(
            packed.len() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        gpu.upload(buffer, &packed)?;
        let record = gpu.storage_buffer(buffer)?.index;
        let lit = load(gpu, artifacts, "transport_lighting")?;
        let diffuse = load(gpu, artifacts, "transport_diffuse")?;
        Ok(Self {
            buffer,
            record,
            lit,
            diffuse,
            bytes: packed.len(),
            access: Access::HostWrite,
        })
    }

    pub fn update(
        &mut self,
        gpu: &mut GpuDevice,
        transport: &PatchTransport,
        scene: &Scene,
    ) -> Result<()> {
        self.upload(gpu, readout_bytes(transport, scene, None))
    }

    #[cfg(feature = "fidelity-lab")]
    pub fn reference(
        &mut self,
        gpu: &mut GpuDevice,
        transport: &PatchTransport,
        scene: &Scene,
        values: &[[f32; 3]],
    ) -> Result<()> {
        anyhow::ensure!(
            values.len() == transport.graph.links.patches.len(),
            "reference patch count mismatch"
        );
        self.upload(gpu, readout_bytes(transport, scene, Some(values)))
    }

    fn upload(&mut self, gpu: &mut GpuDevice, packed: Vec<u8>) -> Result<()> {
        if packed.len() > self.bytes {
            let replacement = gpu.create_buffer(
                packed.len() as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )?;
            gpu.upload(replacement, &packed)?;
            let index = gpu.storage_buffer(replacement)?.index;
            gpu.retire(Resource::Buffer(self.buffer))?;
            self.buffer = replacement;
            self.record = index;
            self.bytes = packed.len();
        } else {
            gpu.upload(self.buffer, &packed)?;
        }
        self.access = Access::HostWrite;
        Ok(())
    }

    pub fn import(&mut self, graph: &mut Graph) -> (ResourceId, Access) {
        let id = graph.import(
            Resource::Buffer(self.buffer),
            State {
                access: self.access,
                queue: Queue::Graphics,
            },
        );
        self.access = Access::ShaderRead;
        (id, Access::ShaderRead)
    }

    pub fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Buffer(self.buffer))?;
        Ok(())
    }
}

fn readout_bytes(
    transport: &PatchTransport,
    scene: &Scene,
    reference: Option<&[[f32; 3]]>,
) -> Vec<u8> {
    let patches = &transport.graph.links.patches;
    let mut cells = BTreeMap::<(u32, [i32; 3]), Vec<u32>>::new();
    for (i, stamp) in patches.stamps.iter().enumerate() {
        let cell = (Vec3::from(patches.position(i as u32)) / 0.25)
            .floor()
            .as_ivec3()
            .to_array();
        cells
            .entry((stamp.id.instance as u32 + 1, cell))
            .or_default()
            .push(i as u32);
    }
    // Readout volume cells use entity zero and six 48-byte axis records. Surface
    // rows use the native instance ID and never interpolate with these cells.
    let volume_cells = transport.volume.records().count();
    let records_offset = 16 + (cells.len() + volume_cells) * 32;
    let mut bytes = Vec::with_capacity(records_offset + patches.len() * 48);
    bytes.extend_from_slice(bytemuck::cast_slice(&[
        (cells.len() + volume_cells) as u32,
        records_offset as u32,
        patches.len() as u32,
        0,
    ]));
    let mut first = 0u32;
    for (cell, _) in transport.volume.records() {
        bytes.extend_from_slice(bytemuck::cast_slice(&[
            cell[0] as u32,
            cell[1] as u32,
            cell[2] as u32,
            0,
            first,
            6,
            0,
            0,
        ]));
        first += 6;
    }
    for (&(instance, cell), indices) in &cells {
        bytes.extend_from_slice(bytemuck::cast_slice(&[
            cell[0] as u32,
            cell[1] as u32,
            cell[2] as u32,
            instance,
            first,
            indices.len() as u32,
            0,
            0,
        ]));
        first += indices.len() as u32;
    }
    for (cell, lobes) in transport.volume.records() {
        for (axis, value) in lobes.iter().enumerate() {
            let pos = glam::IVec3::from_array(cell).as_vec3() * 0.25;
            let normal = [
                Vec3::X,
                Vec3::NEG_X,
                Vec3::Y,
                Vec3::NEG_Y,
                Vec3::Z,
                Vec3::NEG_Z,
            ][axis];
            let row = [
                pos.x, pos.y, pos.z, 0., normal.x, normal.y, normal.z, 0., value.x, value.y,
                value.z, 0.,
            ];
            bytes.extend_from_slice(bytemuck::cast_slice(&row));
        }
    }
    let channel_offset = records_offset + (patches.len() + volume_cells * 6) * 48;
    let mut channel_rows = Vec::<[f32; 8]>::new();
    for indices in cells.values() {
        for &p in indices {
            let pos = patches.position(p);
            let normal = patches.normal.each_ref().map(|c| c[p as usize]);
            let e = reference
                .map_or_else(
                    || transport.irradiance(p, scene),
                    |values| {
                        let links = &transport.graph.links;
                        let mut incoming = Vec3::ZERO;
                        for link in links.receiver_links(p) {
                            let i = link as usize;
                            if let Some(value) = values.get(links.source[i] as usize) {
                                incoming += Vec3::from(*value)
                                    * Vec3::from_array(links.transmission.each_ref().map(|c| c[i]))
                                    * links.weight[i];
                            }
                        }
                        incoming * std::f32::consts::PI + transport.external_irradiance[p as usize]
                    },
                )
                .to_array();
            let channel_start = channel_rows.len();
            if reference.is_none() {
                channel_rows.extend(
                    transport
                        .channels
                        .readout(p)
                        .map(|(e, g)| [e.x, e.y, e.z, 0., g.x, g.y, g.z, 0.]),
                );
            }
            let channel_count = channel_rows.len() - channel_start;
            let row = [
                pos[0],
                pos[1],
                pos[2],
                f32::from_bits((channel_offset + channel_start * 32) as u32),
                normal[0],
                normal[1],
                normal[2],
                f32::from_bits(channel_count as u32),
                e[0],
                e[1],
                e[2],
                0.,
            ];
            bytes.extend_from_slice(bytemuck::cast_slice(&row));
        }
    }
    bytes.extend_from_slice(bytemuck::cast_slice(&channel_rows));
    bytes
}

/// Actual coalesced A2 buffers, not a synthetic differently packed stand-in.
pub(crate) fn gather_bytes(
    links: &LinkStore,
    solver: &EventSolver,
    tiles: &[u32],
    epoch: u32,
) -> Vec<u8> {
    let mut bytes = vec![0u8; 64];
    let mut append = |value: &[u8]| {
        let offset = bytes.len() as u32;
        bytes.extend_from_slice(value);
        offset
    };
    let source = append(bytemuck::cast_slice(&links.source));
    let weight = append(bytemuck::cast_slice(&links.weight));
    let transmission: Vec<[f32; 3]> = (0..links.len())
        .map(|i| links.transmission.each_ref().map(|c| c[i]))
        .collect();
    let transmission = append(bytemuck::cast_slice(&transmission));
    let groups = append(bytemuck::cast_slice(&links.groups));
    let tile = append(bytemuck::cast_slice(&links.tiles));
    let active = append(bytemuck::cast_slice(tiles));
    let albedo = append(bytemuck::cast_slice(&solver.albedo));
    let forcing = append(bytemuck::cast_slice(&solver.forcing));
    let generations = append(bytemuck::cast_slice(&vec![epoch; links.tiles.len()]));
    let header = [
        links.patches.len() as u32,
        links.source_count() as u32,
        links.tiles.len() as u32,
        epoch,
        source,
        weight,
        transmission,
        groups,
        tile,
        active,
        albedo,
        forcing,
        generations,
        tiles.len() as u32,
        0,
        0,
    ];
    bytes[..64].copy_from_slice(bytemuck::cast_slice(&header));
    bytes
}

#[cfg(test)]
pub(super) mod tests;
