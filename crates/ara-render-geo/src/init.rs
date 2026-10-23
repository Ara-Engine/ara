use super::*;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuInstance {
    source: Instance,
    rotation: [f32; 4],
}
impl GeometryView {
    /// Prepares immutable device-local geometry and warms offline pipelines.
    /// Preparation is synchronous and belongs on an extraction worker.
    pub fn new(
        gpu: &mut VulkanDevice,
        geometry: &Geometry,
        instances: &[Instance],
        size: [u32; 2],
        capacity: u32,
        cooked: &Path,
        mesh_shaders: bool,
    ) -> Result<Self> {
        Self::new_with_pool(
            gpu,
            geometry,
            instances,
            GeometryConfig {
                size,
                visible_clusters: capacity,
                resident_pages: u32::try_from(geometry.clusters.len())?,
                mesh_shaders,
            },
            cooked,
        )
    }
    /// Prepares a bounded resident page pool. Budget misses render a complete
    /// coarser DAG cut and report its residency floor rather than leaving holes.
    pub fn new_with_pool(
        gpu: &mut VulkanDevice,
        geometry: &Geometry,
        instances: &[Instance],
        config: GeometryConfig,
        cooked: &Path,
    ) -> Result<Self> {
        let GeometryConfig {
            size,
            visible_clusters: capacity,
            mesh_shaders,
            resident_pages,
        } = config;
        geometry.validate()?;
        ara_texture_stream::validate_device(gpu)?;
        ensure!(
            geometry.clusters.len() <= u32::MAX as usize / 2048
                && geometry.clusters.len() as u64 * instances.len() as u64 <= u64::from(u32::MAX),
            "geometry dispatch/address range exceeds ABI"
        );
        ensure!(
            !size.contains(&0)
                && size.iter().all(|v| *v <= 8192)
                && !instances.is_empty()
                && capacity > 0
                && capacity < (1 << 25),
            "invalid geometry view capacities"
        );
        ensure!(
            gpu.context().info.buffer_int64_atomics,
            "ARA_GEO_CAPABILITY: shaderBufferInt64Atomics required for atomic visibility"
        );
        ensure!(
            instances.iter().all(|i| i
                .position_scale
                .iter()
                .chain(&i.color_roughness)
                .all(|v| v.is_finite())
                && i.position_scale[3] > 0.0
                && f32::from_bits(i.identity[3]).is_finite()),
            "invalid instance transform/material"
        );
        let mesh =
            mesh_shaders && gpu.context().info.mesh_shading && gpu.context().info.geometry_shader;
        if mesh {
            let limits = gpu.context().info.mesh_dispatch;
            let x = capacity.min(65535);
            let y = capacity.div_ceil(65535);
            ensure!(
                x <= limits[0]
                    && y <= limits[1]
                    && u64::from(x) * u64::from(y) <= u64::from(limits[3]),
                "visible cluster capacity exceeds mesh dispatch limits"
            );
        }
        let (residency, pages) = streaming::Residency::new(gpu, geometry, resident_pages)?;
        let mut clusters = geometry.clusters.to_vec();
        for (id, c) in clusters.iter_mut().enumerate() {
            let base = id * 2048;
            c.offset = u32::try_from(base)?;
        }
        let mut low = [f32::INFINITY; 3];
        let mut high = [f32::NEG_INFINITY; 3];
        for c in &clusters {
            for a in 0..3 {
                low[a] = low[a].min(c.bounds.center[a] - c.bounds.radius);
                high[a] = high[a].max(c.bounds.center[a] + c.bounds.radius);
            }
        }
        let center = std::array::from_fn::<_, 3, _>(|i| low[i] * 0.5 + high[i] * 0.5);
        let radius = clusters
            .iter()
            .map(|c| {
                (Vec3::from_array(c.bounds.center) - Vec3::from_array(center)).length()
                    + c.bounds.radius
            })
            .fold(0.0, f32::max);
        let bounds = [center[0], center[1], center[2], radius];
        let pixels = size[0] as usize * size[1] as usize;
        let hzb = size[0].div_ceil(16) as usize * size[1].div_ceil(16) as usize;
        let prepared: Vec<_> = instances
            .iter()
            .map(|&source| {
                let (sin, cos) = f32::from_bits(source.identity[3]).sin_cos();
                GpuInstance {
                    source,
                    rotation: [cos, sin, 0., 0.],
                }
            })
            .collect();
        let mut buffers = Vec::new();
        for (index, bytes) in [
            bytemuck::cast_slice(&prepared).to_vec(),
            bytemuck::cast_slice(&clusters).to_vec(),
            if geometry.groups.is_empty() {
                vec![0; 32]
            } else {
                bytemuck::cast_slice(&geometry.groups).to_vec()
            },
            pages,
            vec![0; instances.len() * 4],
            vec![0; capacity as usize * 8],
            vec![0; 128],
            vec![0; pixels * 8],
            vec![0; hzb * 4],
            vec![0; (clusters.len() + 8) * 4],
            vec![0; pixels * 4],
            vec![0; pixels * 8],
            bytemuck::cast_slice(&bounds).to_vec(),
        ]
        .into_iter()
        .enumerate()
        {
            buffers.push(upload(
                gpu,
                &bytes,
                if index == 6 {
                    vk::BufferUsageFlags::INDIRECT_BUFFER
                } else {
                    vk::BufferUsageFlags::empty()
                },
            )?);
        }
        let frame = gpu.create_buffer(
            size_of::<Frame>() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        gpu.upload(frame, bytemuck::bytes_of(&Frame::zeroed()))?;
        let mut slots = Vec::new();
        for &b in &buffers {
            slots.push(gpu.storage_buffer(b)?.index);
        }
        let mut images = Vec::new();
        let mut image_slots = [0; 4];
        for (i, format) in [
            vk::Format::R32G32B32A32_SFLOAT,
            vk::Format::R32G32B32A32_SFLOAT,
            vk::Format::R32G32B32A32_SFLOAT,
            vk::Format::R8G8B8A8_UNORM,
        ]
        .into_iter()
        .enumerate()
        {
            let image = gpu.create_image(
                size[0],
                size[1],
                format,
                vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )?;
            image_slots[i] = gpu.storage_image(image)?.index;
            images.push(image);
        }
        let depth = gpu.create_image(
            size[0],
            size[1],
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        )?;
        let mut pipelines = BTreeMap::new();
        for name in [
            "reset",
            "instances",
            "select",
            "arguments",
            "software",
            "hardware",
            "hzb",
            "resolve",
            "resolve_textured",
            "stream_upload",
        ] {
            let filename = if name == "hardware" && mesh {
                "mesh"
            } else {
                name
            };
            let manifest: CookedPipeline =
                serde_json::from_slice(&std::fs::read(cooked.join(format!("{filename}.json")))?)?;
            pipelines.insert(name.into(), gpu.create_pipeline(&manifest, cooked)?);
        }
        let mut result = Self {
            frame,
            buffers,
            slots,
            images: images
                .try_into()
                .map_err(|_| anyhow::anyhow!("surface count"))?,
            depth,
            image_slots,
            pipelines,
            previous: Vec::new(),
            previous_vp: Mat4::IDENTITY,
            size,
            instance_count: u32::try_from(instances.len())?,
            cluster_count: u32::try_from(clusters.len())?,
            group_count: u32::try_from(geometry.groups.len())?,
            capacity,
            min: geometry.min,
            scale: geometry.scale,
            mesh,
            last: None,
            source_triangles: geometry.diagnostics.source_triangles * instances.len() as u64,
            residency,
            textures: None,
            debug: DebugView::Lit,
            texture_limit: instances
                .iter()
                .filter(|i| i.identity[2] != u32::MAX)
                .map(|i| i.identity[2] + 1)
                .max()
                .unwrap_or(0),
        };
        result.slots.push(gpu.storage_buffer(frame)?.index);
        Ok(result)
    }
}
use ara_gpu::vk;
pub(super) fn upload(
    gpu: &mut VulkanDevice,
    bytes: &[u8],
    extra: vk::BufferUsageFlags,
) -> Result<Buffer> {
    let b = gpu.create_buffer(
        bytes.len() as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::TRANSFER_DST
            | vk::BufferUsageFlags::TRANSFER_SRC
            | extra,
        false,
    )?;
    let stage = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(stage, bytes)?;
    let mut r = gpu.begin(Queue::Graphics);
    r.push(Command::CopyBuffer {
        source: stage,
        destination: b,
        size: bytes.len() as u64,
    });
    r.barriers([ara_gpu::Barrier {
        resource: Resource::Buffer(b),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let p = gpu.submit(r.finish(), &[])?;
    gpu.resolve(p)?;
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    Ok(b)
}
