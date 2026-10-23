use super::*;
impl Renderer2d {
    /// Uploads an immutable extraction and prewarms its offline-cooked graphics pipeline.
    pub fn new(scene: &Scene2d, size: [u32; 2], cache: impl Into<PathBuf>) -> Result<Self> {
        ensure!(
            !size.contains(&0) && size.iter().all(|v| *v <= 8192),
            "invalid 2D target extent"
        );
        ensure!(
            scene.lights.len() <= 64 && scene.occluders.len() <= 4096,
            "2D light/occluder capacity exceeded"
        );
        let mut gpu = VulkanDevice::new(&[], cache)?;
        let artifacts = Path::new(env!("CARGO_MANIFEST_DIR")).join("cooked");
        let cooked: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(artifacts.join("sprites.json"))?)?;
        let pipeline = gpu.create_pipeline(&cooked, &artifacts)?;
        let shaders = ShaderMetadata {
            content_key: cooked.key,
            compiler: cooked.compiler,
            binding_layout_hash: cooked.binding_layout_hash,
        };
        let mut indices = BTreeMap::new();
        let mut images = Vec::new();
        let flat = upload_image(
            &mut gpu,
            [1, 1],
            vk::Format::R8G8B8A8_UNORM,
            &[128, 128, 255, 255],
        )?;
        indices.insert(
            "__flat_normal".into(),
            gpu.sampled_image(flat, false)?.index,
        );
        images.push(flat);
        for (id, atlas) in &scene.atlases {
            ensure!(
                id != "__flat_normal"
                    && ara_schema_ref(id)
                    && (1..=8192).contains(&atlas.atlas.width)
                    && (1..=8192).contains(&atlas.atlas.height)
                    && atlas.rgba.len()
                        == atlas.atlas.width as usize * atlas.atlas.height as usize * 4,
                "invalid resident atlas/reference"
            );
            for region in atlas.atlas.regions.values() {
                ensure!(
                    region.width > 0
                        && region.height > 0
                        && region
                            .x
                            .checked_add(region.width)
                            .is_some_and(|x| x <= atlas.atlas.width)
                        && region
                            .y
                            .checked_add(region.height)
                            .is_some_and(|y| y <= atlas.atlas.height)
                        && region
                            .pivot
                            .iter()
                            .chain(region.borders.iter())
                            .all(|v| v.is_finite())
                        && region.borders.iter().all(|v| *v >= 0.)
                        && region.borders[0] + region.borders[2] <= region.width as f32
                        && region.borders[1] + region.borders[3] <= region.height as f32,
                    "invalid atlas region"
                );
            }
            let image = upload_image(
                &mut gpu,
                [atlas.atlas.width, atlas.atlas.height],
                if atlas.linear {
                    vk::Format::R8G8B8A8_UNORM
                } else {
                    vk::Format::R8G8B8A8_SRGB
                },
                &atlas.rgba,
            )?;
            indices.insert(id.to_owned(), gpu.sampled_image(image, false)?.index);
            images.push(image);
        }
        let prepared = batch::prepare(scene, &indices)?;
        let instance_count = prepared.instances.len();
        let instance_bytes: &[u8] = bytemuck::cast_slice(&prepared.instances);
        let instances = upload_buffer(
            &mut gpu,
            if instance_bytes.is_empty() {
                &[0u8; 128]
            } else {
                instance_bytes
            },
        )?;
        let instance_index = gpu.storage_buffer(instances)?.index;
        let mut scene_frame = vec![0u8; 64];
        for light in &scene.lights {
            ensure!(
                light
                    .position
                    .iter()
                    .chain(light.color.iter())
                    .all(|v| v.is_finite())
                    && light.radius.is_finite()
                    && light.radius > 0.,
                "invalid 2D light"
            );
            scene_frame.extend(bytemuck::cast_slice(&[
                light.position[0],
                light.position[1],
                light.position[2],
                light.radius,
                light.color[0],
                light.color[1],
                light.color[2],
                f32::from(u8::from(light.shadows)),
            ]));
        }
        for segment in &scene.occluders {
            ensure!(
                segment.iter().all(|v| v.is_finite()),
                "invalid shadow segment"
            );
            scene_frame.extend(bytemuck::bytes_of(segment));
        }
        let counts = [
            scene.lights.len() as u32,
            scene.occluders.len() as u32,
            0,
            0,
        ];
        scene_frame[32..48].copy_from_slice(bytemuck::bytes_of(&counts));
        scene_frame[48..64].copy_from_slice(bytemuck::bytes_of(&[
            64u32,
            64 + scene.lights.len() as u32 * 32,
            0,
            0,
        ]));
        let frame = gpu.create_buffer(
            scene_frame.len() as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        let hdr = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R32G32B32A32_SFLOAT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
        )?;
        let ids = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R32_SFLOAT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let output = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let hdr_index = gpu.sampled_image(hdr, false)?.index;
        let output_index = gpu.storage_image(output)?.index;
        let readbacks = [
            gpu.create_buffer(
                u64::from(size[0]) * u64::from(size[1]) * 4,
                vk::BufferUsageFlags::TRANSFER_DST,
                true,
            )?,
            gpu.create_buffer(
                u64::from(size[0]) * u64::from(size[1]) * 4,
                vk::BufferUsageFlags::TRANSFER_DST,
                true,
            )?,
        ];
        let post = ToneMap::new(&mut gpu)?;
        let hash = blake3::hash(&serde_json::to_vec(scene)?)
            .to_hex()
            .to_string();
        Ok(Self {
            gpu,
            pipeline,
            post,
            instances,
            instance_index,
            frame,
            frame_index,
            images,
            targets: [hdr, ids, output],
            states: [State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            }; 3],
            hdr_index,
            output_index,
            readbacks,
            batches: prepared.batches,
            size,
            scene_frame,
            names: prepared.names,
            hash,
            shaders,
            timings: Vec::new(),
            instance_count,
        })
    }
}
fn ara_schema_ref(s: &str) -> bool {
    ara_schema::grammar::parse_ref(s).is_ok_and(|r| r.id.is_some())
}
fn upload_buffer(gpu: &mut VulkanDevice, bytes: &[u8]) -> Result<Buffer> {
    let source = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(source, bytes)?;
    let buffer = gpu.create_buffer(
        bytes.len() as u64,
        vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::STORAGE_BUFFER,
        false,
    )?;
    let mut r = gpu.begin(Queue::Graphics);
    r.push(Command::CopyBuffer {
        source,
        destination: buffer,
        size: bytes.len() as u64,
    });
    r.barriers([Barrier {
        resource: Resource::Buffer(buffer),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let point = gpu.submit(r.finish(), &[])?;
    gpu.resolve(point)?;
    gpu.retire(Resource::Buffer(source))?;
    gpu.collect()?;
    Ok(buffer)
}
fn upload_image(
    gpu: &mut VulkanDevice,
    size: [u32; 2],
    format: vk::Format,
    bytes: &[u8],
) -> Result<Image> {
    let image = gpu.create_image(
        size[0],
        size[1],
        format,
        vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
    )?;
    let source = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    gpu.upload(source, bytes)?;
    let mut r = gpu.begin(Queue::Graphics);
    r.barriers([Barrier {
        resource: Resource::Image(image),
        before: Access::Undefined,
        after: Access::CopyDestination,
        handoff: None,
    }]);
    r.push(Command::UploadImage {
        source,
        destination: image,
    });
    r.barriers([Barrier {
        resource: Resource::Image(image),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let point = gpu.submit(r.finish(), &[])?;
    gpu.resolve(point)?;
    gpu.retire(Resource::Buffer(source))?;
    gpu.collect()?;
    Ok(image)
}
