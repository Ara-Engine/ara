use crate::gpu::{Buffer, Gpu, Image, image_barrier};
use anyhow::Result;
use ash::vk;
use bytemuck::{Pod, Zeroable};

/// Interleaved position/normal/UV input shared by raster and AS construction.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Static geometry/textures and the four-buffer compute-to-graphics ring.
pub struct Scene {
    pub vertices: Buffer,
    pub indices: Buffer,
    pub vertex_count: u32,
    pub draws: Vec<Draw>,
    pub textures: Vec<Image>,
    pub material_buffers: [Buffer; 4],
}

/// Index range with its material/texture index in the global heap.
pub struct Draw {
    pub first_index: u32,
    pub count: u32,
    pub material: u32,
}

/// Deterministic scene shared with the Mac port: ground plus three boxes.
pub fn create(gpu: &mut Gpu) -> Result<Scene> {
    let mut vertices = vec![];
    let mut indices = vec![];
    let mut draws = vec![];
    quad(
        &mut vertices,
        &mut indices,
        [[-5., 0., -5.], [-5., 0., 5.], [5., 0., 5.], [5., 0., -5.]],
        [0., 1., 0.],
        10.,
    );
    draws.push(Draw {
        first_index: 0,
        count: 6,
        material: 0,
    });
    for (material, center, half) in [
        (1, [-1.8, 0.85, 0.], [0.75, 0.85, 0.75]),
        (2, [0.35, 1.25, -0.8], [0.8, 1.25, 0.8]),
        (3, [1.65, 0.5, 1.25], [0.65, 0.5, 0.65]),
    ] {
        let first_index = indices.len() as u32;
        cube(&mut vertices, &mut indices, center, half);
        draws.push(Draw {
            first_index,
            count: indices.len() as u32 - first_index,
            material,
        });
    }
    let input_usage = vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
        | vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR;
    let vertex_buffer = gpu.buffer(
        std::mem::size_of_val(vertices.as_slice()) as u64,
        input_usage | vk::BufferUsageFlags::VERTEX_BUFFER,
        true,
    )?;
    let index_buffer = gpu.buffer(
        std::mem::size_of_val(indices.as_slice()) as u64,
        input_usage | vk::BufferUsageFlags::INDEX_BUFFER,
        true,
    )?;
    gpu.upload(vertex_buffer, bytemuck::cast_slice(&vertices))?;
    gpu.upload(index_buffer, bytemuck::cast_slice(&indices))?;
    let mut textures = vec![];
    for palette in [
        ([95, 115, 129], [47, 60, 72]),
        ([215, 71, 38], [111, 27, 19]),
        ([21, 156, 168], [12, 68, 91]),
        ([232, 174, 40], [125, 74, 16]),
    ] {
        textures.push(texture(gpu, palette)?);
    }
    let usage = vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC;
    let material_buffers = [
        gpu.buffer(4096, usage, false)?,
        gpu.buffer(4096, usage, false)?,
        gpu.buffer(4096, usage, false)?,
        gpu.buffer(4096, usage, false)?,
    ];
    Ok(Scene {
        vertices: vertex_buffer,
        indices: index_buffer,
        vertex_count: vertices.len() as u32,
        draws,
        textures,
        material_buffers,
    })
}

fn quad(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    positions: [[f32; 3]; 4],
    normal: [f32; 3],
    tile: f32,
) {
    let base = vertices.len() as u32;
    for (position, uv) in
        positions
            .into_iter()
            .zip([[0., 0.], [0., tile], [tile, tile], [tile, 0.]])
    {
        vertices.push(Vertex {
            position,
            normal,
            uv,
        });
    }
    indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn cube(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>, c: [f32; 3], h: [f32; 3]) {
    let [x, y, z] = c;
    let [a, b, d] = h;
    for (positions, normal) in [
        (
            [
                [x - a, y - b, z + d],
                [x + a, y - b, z + d],
                [x + a, y + b, z + d],
                [x - a, y + b, z + d],
            ],
            [0., 0., 1.],
        ),
        (
            [
                [x + a, y - b, z - d],
                [x - a, y - b, z - d],
                [x - a, y + b, z - d],
                [x + a, y + b, z - d],
            ],
            [0., 0., -1.],
        ),
        (
            [
                [x + a, y - b, z + d],
                [x + a, y - b, z - d],
                [x + a, y + b, z - d],
                [x + a, y + b, z + d],
            ],
            [1., 0., 0.],
        ),
        (
            [
                [x - a, y - b, z - d],
                [x - a, y - b, z + d],
                [x - a, y + b, z + d],
                [x - a, y + b, z - d],
            ],
            [-1., 0., 0.],
        ),
        (
            [
                [x - a, y + b, z + d],
                [x + a, y + b, z + d],
                [x + a, y + b, z - d],
                [x - a, y + b, z - d],
            ],
            [0., 1., 0.],
        ),
        (
            [
                [x - a, y - b, z - d],
                [x + a, y - b, z - d],
                [x + a, y - b, z + d],
                [x - a, y - b, z + d],
            ],
            [0., -1., 0.],
        ),
    ] {
        quad(vertices, indices, positions, normal, 2.);
    }
}

fn texture(gpu: &mut Gpu, palette: ([u8; 3], [u8; 3])) -> Result<Image> {
    let mut pixels = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64 {
        for x in 0..64 {
            let color = if (x / 8 + y / 8) % 2 == 0 {
                palette.0
            } else {
                palette.1
            };
            pixels.extend([color[0], color[1], color[2], 255]);
        }
    }
    let upload = gpu.buffer(
        pixels.len() as u64,
        vk::BufferUsageFlags::TRANSFER_SRC,
        true,
    )?;
    gpu.upload(upload, &pixels)?;
    let image = gpu.image(
        64,
        64,
        vk::Format::R8G8B8A8_UNORM,
        vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
        vk::ImageAspectFlags::COLOR,
    )?;
    gpu.immediate(|cmd| {
        // SAFETY: Startup-only graphics commands with live staging/image resources;
        // barriers transition the image before each use, and immediate waits idle.
        unsafe {
            let barrier = image_barrier(
                image.handle,
                vk::ImageAspectFlags::COLOR,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_WRITE,
                ),
            );
            gpu.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&[barrier]),
            );
            let copy = vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: 64,
                    height: 64,
                    depth: 1,
                });
            gpu.device.cmd_copy_buffer_to_image(
                cmd,
                upload.handle,
                image.handle,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            );
            let barrier = image_barrier(
                image.handle,
                vk::ImageAspectFlags::COLOR,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    vk::AccessFlags2::SHADER_SAMPLED_READ,
                ),
            );
            gpu.device.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&[barrier]),
            );
        }
    })?;
    Ok(image)
}
