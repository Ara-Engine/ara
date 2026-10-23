use super::*;
impl Landscape {
    /// Creates static device-local scene inputs and prewarmed offline pipelines.
    pub fn new(
        gpu: &mut VulkanDevice,
        data: &LandscapeData,
        size: [u32; 2],
        cooked: &Path,
    ) -> Result<Self> {
        ara_texture_stream::validate_device(gpu)?;
        data.header
            .validate()
            .map_err(|d| anyhow::anyhow!(d.message))?;
        ensure!(
            data.instances.iter().all(|i| i.identity[2] < 3
                && i.position_scale
                    .iter()
                    .chain(&i.rotation_wind)
                    .all(|v| v.is_finite())
                && i.position_scale[3] > 0.
                && i.rotation_wind[3] >= 0.),
            "invalid foliage instance species, bounds or transform"
        );
        ensure!(
            data.samples.iter().all(|s| f32::from_bits(s[0]).is_finite()
                && s[2] <= 1
                && s[1]
                    .to_le_bytes()
                    .iter()
                    .map(|v| u32::from(*v))
                    .sum::<u32>()
                    == 255),
            "invalid terrain sample atlas"
        );
        ensure!(
            !size.contains(&0)
                && data.side > 1
                && data.header.min.x == 0
                && data.header.min.z == 0
                && data.header.origin == [0., 0.]
                && data.header.max.x == data.header.max.z,
            "GPU landscape expects a square origin-zero extraction"
        );
        ensure!(
            data.samples.len() == (data.side as usize).pow(2)
                && data.side == data.header.cells * data.header.max.x as u32 + 1,
            "invalid sample atlas"
        );
        let count = u32::try_from(data.instances.len())?;
        ensure!(count > 0, "empty instance scene");
        let mut pages = Pages::new(data, [View::noon().eye.x, View::noon().eye.z])?;
        let tiles = data.header.max.x as u32;
        ensure!(
            tiles <= 128 && data.header.cells == 32,
            "GPU patch ABI requires 32 cells and <=128 tiles per edge"
        );
        let mut slots = [0; 9];
        let mut storage =
            |bytes: &[u8], extra: vk::BufferUsageFlags, host: bool| -> Result<Buffer> {
                let buffer = gpu.create_buffer(
                    bytes.len() as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER
                        | extra
                        | if host {
                            vk::BufferUsageFlags::empty()
                        } else {
                            vk::BufferUsageFlags::TRANSFER_DST
                        },
                    host,
                )?;
                if host {
                    gpu.upload(buffer, bytes)?;
                } else {
                    let staging = gpu.create_buffer(
                        bytes.len() as u64,
                        vk::BufferUsageFlags::TRANSFER_SRC,
                        true,
                    )?;
                    gpu.upload(staging, bytes)?;
                    let mut r = gpu.begin(Queue::Graphics);
                    r.push(Command::CopyBuffer {
                        source: staging,
                        destination: buffer,
                        size: bytes.len() as u64,
                    });
                    r.barriers([ara_gpu::Barrier {
                        resource: Resource::Buffer(buffer),
                        before: Access::CopyDestination,
                        after: Access::ShaderRead,
                        handoff: None,
                    }]);
                    let p = gpu.submit(r.finish(), &[])?;
                    gpu.resolve(p)?;
                    gpu.retire(Resource::Buffer(staging))?;
                    gpu.collect()?;
                }
                Ok(buffer)
            };
        let zero_frame: Frame = bytemuck::Zeroable::zeroed();
        let frame = storage(
            bytemuck::bytes_of(&zero_frame),
            vk::BufferUsageFlags::empty(),
            true,
        )?;
        let samples = storage(
            bytemuck::cast_slice(&pages.words),
            vk::BufferUsageFlags::empty(),
            false,
        )?;
        let instances = storage(
            bytemuck::cast_slice(&data.instances),
            vk::BufferUsageFlags::empty(),
            false,
        )?;
        let levels = storage(
            &vec![0; tiles as usize * tiles as usize * 4],
            vk::BufferUsageFlags::empty(),
            false,
        )?;
        let lists = storage(
            &vec![0; 6 * tiles as usize * tiles as usize * 4],
            vk::BufferUsageFlags::empty(),
            false,
        )?;
        let args = storage(
            &[0; 6 * 16],
            vk::BufferUsageFlags::INDIRECT_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            false,
        )?;
        let visible = storage(
            &vec![0; count as usize * 9 * 4],
            vk::BufferUsageFlags::empty(),
            false,
        )?;
        let foliage_args = storage(
            &[0; 9 * 16],
            vk::BufferUsageFlags::INDIRECT_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC,
            false,
        )?;
        let parameters = storage(
            bytemuck::cast_slice(&[[0f32; 4]; 5]),
            vk::BufferUsageFlags::empty(),
            true,
        )?;
        let upload = storage(
            &vec![0; (4 + 33 * 33 * 3) * 4 * UPLOAD_BUDGET],
            vk::BufferUsageFlags::empty(),
            true,
        )?;
        let upload_slot = gpu.storage_buffer(upload)?.index;
        pages.words.clear();
        pages.words.shrink_to_fit();
        for (i, b) in [
            frame,
            samples,
            instances,
            levels,
            lists,
            args,
            visible,
            foliage_args,
            parameters,
        ]
        .into_iter()
        .enumerate()
        {
            slots[i] = gpu.storage_buffer(b)?.index;
        }
        let color = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R8G8B8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let depth = gpu.create_image(
            size[0],
            size[1],
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                | vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;
        let shadow = gpu.create_image(
            2048,
            2048,
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
        )?;
        let hzb = gpu.create_image(
            size[0].div_ceil(32),
            size[1].div_ceil(32),
            vk::Format::R32G32B32A32_SFLOAT,
            vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED,
        )?;
        let image_slots = [
            gpu.sampled_image(depth, true)?.index,
            gpu.sampled_image(shadow, true)?.index,
            gpu.sampled_image(hzb, false)?.index,
            gpu.storage_image(hzb)?.index,
        ];
        let mut textures = Vec::new();
        let mut texture_slots = Vec::new();
        for species in 0..4 {
            let mut pixels = Vec::<u8>::new();
            for y in 0..128 {
                for x in 0..128 {
                    let n = (ara_foliage::hash((x + y * 128 + species * 16384) as u64) >> 56)
                        as f32
                        / 255.;
                    let base = match species {
                        0 => [0.26, 0.32, 0.12],
                        1 => [0.39, 0.36, 0.31],
                        2 => [0.32, 0.21, 0.11],
                        _ => [0.85, 0.88, 0.92],
                    };
                    for v in base {
                        pixels.push(((v * (0.7 + n * 0.6)).clamp(0., 1.) * 255.) as u8);
                    }
                    pixels.push(255);
                }
            }
            let image = gpu.create_image(
                128,
                128,
                vk::Format::R8G8B8A8_UNORM,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
            )?;
            let stage = gpu.create_buffer(
                pixels.len() as u64,
                vk::BufferUsageFlags::TRANSFER_SRC,
                true,
            )?;
            gpu.upload(stage, &pixels)?;
            let mut r = gpu.begin(Queue::Graphics);
            r.barriers([ara_gpu::Barrier {
                resource: Resource::Image(image),
                before: Access::Undefined,
                after: Access::CopyDestination,
                handoff: None,
            }]);
            r.push(Command::UploadImage {
                source: stage,
                destination: image,
            });
            r.barriers([ara_gpu::Barrier {
                resource: Resource::Image(image),
                before: Access::CopyDestination,
                after: Access::ShaderRead,
                handoff: None,
            }]);
            let p = gpu.submit(r.finish(), &[])?;
            gpu.resolve(p)?;
            gpu.retire(Resource::Buffer(stage))?;
            gpu.collect()?;
            let slot = gpu.sampled_image(image, false)?.index;
            texture_slots.push(slot);
            textures.push(image);
        }
        gpu.upload(
            parameters,
            bytemuck::cast_slice(&[
                [texture_slots[0] as f32, 0., 0., 0.],
                [texture_slots[1] as f32, 0., 0., 0.],
                [texture_slots[2] as f32, 0., 0., 0.],
                [texture_slots[3] as f32, 0., 0., 0.],
                [0.12, 0., 0., 0.],
            ]),
        )?;
        let mut pipelines = BTreeMap::new();
        for name in [
            "tile_upload",
            "reset",
            "select",
            "terrain_cull",
            "terrain_depth",
            "terrain_color",
            "terrain_shadow",
            "hzb",
            "foliage_cull",
            "foliage_depth",
            "foliage_color",
            "foliage_shadow",
            "sky",
        ] {
            let p: CookedPipeline =
                serde_json::from_slice(&std::fs::read(cooked.join(format!("{name}.json")))?)?;
            pipelines.insert(name.into(), gpu.create_pipeline(&p, cooked)?);
        }
        Ok(Self {
            frame,
            samples,
            instances,
            levels,
            lists,
            args,
            visible,
            foliage_args,
            parameters,
            upload,
            slots,
            pipelines,
            color,
            depth,
            shadow,
            hzb,
            image_slots,
            textures,
            size,
            count,
            side: data.side,
            cells: data.header.cells,
            tiles,
            spacing: data.header.spacing,
            previous: Vec::new(),
            pages,
            upload_slot,
            pending_uploads: 0,
            upload_words: Vec::with_capacity((4 + 33 * 33 * 3) * UPLOAD_BUDGET),
            geometry: None,
            geometry_slot: 0,
            virtual_textures: None,
        })
    }
    /// Installs authored mesh LODs and baked alpha impostors off the frame path.
    /// Resolve Rule.asset into this catalogue and call apply_bounds before scene
    /// extraction. Textures/geometry are retired with this landscape snapshot.
    pub fn install_geometry(
        &mut self,
        gpu: &mut VulkanDevice,
        geometry: &crate::FoliageGeometry,
    ) -> Result<()> {
        ensure!(
            self.geometry.is_none(),
            "install geometry on a fresh replacement landscape snapshot"
        );
        let mut images = Vec::new();
        let mut slots = Vec::new();
        for texture in &geometry.textures {
            let (image, slot) = crate::upload::texture(gpu, texture)?;
            images.push(image);
            slots.push(slot);
        }
        let bytes = geometry.pack(&slots)?;
        let buffer = crate::upload::buffer(gpu, &bytes)?;
        self.geometry_slot = gpu.storage_buffer(buffer)?.index;
        self.geometry = Some(buffer);
        self.textures.extend(images);
        Ok(())
    }
}
