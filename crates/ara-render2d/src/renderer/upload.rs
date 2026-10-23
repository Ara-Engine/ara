use super::*;
/// Minimum instance allocation: one 128-byte instance.
const INSTANCE: u64 = 128;
/// Frame header: view, ambient, counts and offsets (4 x 16 bytes).
const HEADER: usize = 64;

impl Renderer2d {
    /// Uploads atlases and an extraction, and prewarms the offline-cooked pipeline.
    /// Stage this off the human's frame (D20); later frames only `update` or `render`.
    pub fn new(scene: &Scene2d, size: [u32; 2], cache: impl Into<PathBuf>) -> Result<Self> {
        Self::with_extensions(scene, size, cache, &[])
    }

    /// Creates the same renderer with the instance extensions required by an SDL
    /// window. Existing headless and capture callers continue to use `new`.
    pub fn with_extensions(
        scene: &Scene2d,
        size: [u32; 2],
        cache: impl Into<PathBuf>,
        extensions: &[std::ffi::CString],
    ) -> Result<Self> {
        ensure!(
            !size.contains(&0) && size.iter().all(|v| *v <= 8192),
            "invalid 2D target extent"
        );
        let mut gpu = VulkanDevice::new(extensions, cache)?;
        let artifacts = ara_gpu::cooked::resource_dir(
            env!("CARGO_PKG_NAME"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
        );
        let cooked: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(artifacts.join("sprites.json"))?)?;
        let pipeline = gpu.create_pipeline(&cooked, &artifacts)?;
        let shaders = ShaderMetadata {
            content_key: cooked.key,
            compiler: cooked.compiler,
            binding_layout_hash: cooked.binding_layout_hash,
        };
        let (images, indices) = upload_atlases(&mut gpu, scene)?;
        let mut atlas_digest = blake3::Hasher::new();
        for (id, atlas) in &scene.atlases {
            atlas_digest.update(id.as_bytes());
            atlas_digest.update(&serde_json::to_vec(&atlas.atlas)?);
            atlas_digest.update(&[u8::from(atlas.linear)]);
            atlas_digest.update(&atlas.rgba);
        }
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
        let pixels = u64::from(size[0]) * u64::from(size[1]) * 4;
        let readbacks = [
            gpu.create_buffer(pixels, vk::BufferUsageFlags::TRANSFER_DST, true)?,
            gpu.create_buffer(pixels, vk::BufferUsageFlags::TRANSFER_DST, true)?,
        ];
        let post = ToneMap::new(&mut gpu)?;
        let (instances, staging, instance_index) = instance_buffers(&mut gpu, INSTANCE)?;
        let frame = frame_buffer(&mut gpu, HEADER as u64)?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        let mut renderer = Self {
            gpu,
            pipeline,
            post,
            instances,
            staging,
            instance_capacity: INSTANCE,
            instance_index,
            instance_state: Access::Undefined,
            frame,
            frame_capacity: HEADER as u64,
            frame_index,
            images,
            indices,
            atlas_key: atlas_key(scene),
            atlas_digest: *atlas_digest.finalize().as_bytes(),
            targets: [hdr, ids, output],
            states: [State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            }; 3],
            hdr_index,
            output_index,
            readbacks,
            batches: Vec::new(),
            size,
            scene_frame: vec![0; HEADER],
            instance_bytes: Vec::new(),
            instance_count: 0,
            names: Vec::new(),
            shaders,
            timings: Vec::new(),
        };
        renderer.load(scene)?;
        Ok(renderer)
    }
    /// Replaces the extraction between frames on the same device and pipeline: moved
    /// sprites, flipbook frames, painted tiles, lights. Buffers grow as needed. The atlas
    /// set (references, metadata, encodings) must be unchanged; pixels under an unchanged
    /// reference are assumed unchanged, so hot-reloaded atlases need a new renderer.
    pub fn update(&mut self, scene: &Scene2d) -> Result<()> {
        ensure!(
            self.atlas_key.len() == scene.atlases.len()
                && self.atlas_key.iter().zip(&scene.atlases).all(
                    |((id, atlas, linear), (sid, s))| {
                        id == sid && *atlas == s.atlas && *linear == s.linear
                    }
                ),
            "2D atlas set changed; stage a new Renderer2d"
        );
        self.load(scene)
    }
    fn load(&mut self, scene: &Scene2d) -> Result<()> {
        ensure!(
            scene.lights.len() <= 64 && scene.occluders.len() <= 4096,
            "2D light/occluder capacity exceeded"
        );
        // Validate and build everything before touching the GPU, so a rejected update
        // leaves the previous extraction fully resident and consistent.
        let prepared = batch::prepare(scene, &self.indices, &self.names)?;
        let mut frame = vec![0u8; HEADER];
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
            frame.extend(bytemuck::cast_slice(&[
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
            frame.extend(bytemuck::bytes_of(segment));
        }
        let lights = scene.lights.len() as u32;
        let counts = [lights, scene.occluders.len() as u32, 0, 0];
        frame[32..48].copy_from_slice(bytemuck::bytes_of(&counts));
        frame[48..64].copy_from_slice(bytemuck::bytes_of(&[64u32, 64 + lights * 32, 0, 0]));
        let bytes: &[u8] = if prepared.instances.is_empty() {
            &[0; INSTANCE as usize]
        } else {
            bytemuck::cast_slice(&prepared.instances)
        };
        let needed = bytes.len() as u64;
        if needed > self.instance_capacity {
            self.gpu.retire(Resource::Buffer(self.instances))?;
            self.gpu.retire(Resource::Buffer(self.staging))?;
            let capacity = needed.next_power_of_two();
            (self.instances, self.staging, self.instance_index) =
                instance_buffers(&mut self.gpu, capacity)?;
            self.instance_capacity = capacity;
            self.instance_state = Access::Undefined;
        }
        self.gpu.upload(self.staging, bytes)?;
        let mut r = self.gpu.begin(Queue::Graphics);
        r.barriers([barrier(
            self.instances,
            self.instance_state,
            Access::CopyDestination,
        )]);
        r.push(Command::CopyBuffer {
            source: self.staging,
            destination: self.instances,
            size: needed,
        });
        r.barriers([barrier(
            self.instances,
            Access::CopyDestination,
            Access::ShaderRead,
        )]);
        let point = self.gpu.submit(r.finish(), &[])?;
        self.gpu.resolve(point)?;
        self.gpu.collect()?;
        self.instance_state = Access::ShaderRead;
        if frame.len() as u64 > self.frame_capacity {
            self.gpu.retire(Resource::Buffer(self.frame))?;
            self.frame_capacity = (frame.len() as u64).next_power_of_two();
            self.frame = frame_buffer(&mut self.gpu, self.frame_capacity)?;
            self.frame_index = self.gpu.storage_buffer(self.frame)?.index;
        }
        self.scene_frame = frame;
        self.instance_bytes.clear();
        self.instance_bytes.extend_from_slice(bytes);
        self.instance_count = prepared.instances.len();
        self.batches = prepared.batches;
        if let Some(names) = prepared.names {
            self.names = names;
        }
        Ok(())
    }
    /// Provenance digest of what is resident: atlases, instances, lights and shadows.
    /// Computed only when a capture asks for it, so per-frame updates stay cheap.
    pub(super) fn content_hash(&self) -> Result<blake3::Hasher> {
        let mut hash = blake3::Hasher::new();
        hash.update(&self.atlas_digest);
        hash.update(&self.instance_bytes);
        hash.update(&self.scene_frame[32..]);
        hash.update(&serde_json::to_vec(&self.names)?);
        Ok(hash)
    }
}
fn barrier(buffer: Buffer, before: Access, after: Access) -> Barrier {
    Barrier {
        resource: Resource::Buffer(buffer),
        before,
        after,
        handoff: None,
    }
}
/// Device-local instance storage plus its persistent host-visible staging buffer.
fn instance_buffers(gpu: &mut VulkanDevice, capacity: u64) -> Result<(Buffer, Buffer, u32)> {
    let staging = gpu.create_buffer(capacity, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let instances = gpu.create_buffer(
        capacity,
        vk::BufferUsageFlags::TRANSFER_DST | vk::BufferUsageFlags::STORAGE_BUFFER,
        false,
    )?;
    let index = gpu.storage_buffer(instances)?.index;
    Ok((instances, staging, index))
}
fn frame_buffer(gpu: &mut VulkanDevice, capacity: u64) -> Result<Buffer> {
    Ok(gpu.create_buffer(capacity, vk::BufferUsageFlags::STORAGE_BUFFER, true)?)
}
/// What an `update` may not change without a new renderer.
fn atlas_key(scene: &Scene2d) -> Vec<(String, ara_2d::Atlas, bool)> {
    scene
        .atlases
        .iter()
        .map(|(id, a)| (id.to_owned(), a.atlas.clone(), a.linear))
        .collect()
}
/// Validates and uploads every atlas plus a flat normal; returns bindless indices by ref.
fn upload_atlases(
    gpu: &mut VulkanDevice,
    scene: &Scene2d,
) -> Result<(Vec<Image>, BTreeMap<String, u32>)> {
    let mut indices = BTreeMap::new();
    let mut images = Vec::new();
    let flat = upload_image(
        gpu,
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
                && atlas.rgba.len() == atlas.atlas.width as usize * atlas.atlas.height as usize * 4,
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
            gpu,
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
    Ok((images, indices))
}
fn ara_schema_ref(s: &str) -> bool {
    ara_schema::grammar::parse_ref(s).is_ok_and(|r| r.id.is_some())
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
