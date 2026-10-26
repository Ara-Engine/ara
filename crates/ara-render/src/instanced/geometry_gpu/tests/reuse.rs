use super::*;

pub(super) fn verify_static(
    gpu: &mut VulkanDevice,
    pool: &GeometryGpu,
    scene: &Scene,
) -> Result<()> {
    let size = scene
        .geometry
        .iter()
        .map(|geometry| {
            let vertices = if geometry.deforming {
                0
            } else {
                geometry.vertices.len() * std::mem::size_of::<Vertex>()
            };
            vertices
                + geometry
                    .lods
                    .iter()
                    .map(|lod| lod.indices.len() * 4)
                    .sum::<usize>()
        })
        .sum::<usize>() as u64;
    let source = pool.static_pool.expect("populated fixture");
    let destination = gpu.create_buffer(size, vk::BufferUsageFlags::TRANSFER_DST, true)?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.barriers([
        upload::edge(
            Resource::Buffer(source),
            Access::ShaderRead,
            Access::CopySource,
        ),
        upload::edge(
            Resource::Buffer(destination),
            Access::Undefined,
            Access::CopyDestination,
        ),
    ]);
    commands.push(Command::CopyBuffer {
        source,
        destination,
        size,
    });
    commands.barriers([
        upload::edge(
            Resource::Buffer(source),
            Access::CopySource,
            Access::ShaderRead,
        ),
        upload::edge(
            Resource::Buffer(destination),
            Access::CopyDestination,
            Access::HostRead,
        ),
    ]);
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    let bytes = gpu.read(destination)?;
    for (index, geometry) in scene.geometry.iter().enumerate() {
        let range = pool.ranges[index];
        let first = &pool.lod_data[range.first as usize];
        if !geometry.deforming {
            let vertices: &[u8] = bytemuck::cast_slice(&geometry.vertices);
            let offset = first.vertex_offset as usize;
            assert_eq!(&bytes[offset..offset + vertices.len()], vertices);
        }
        for (index, lod) in geometry.lods.iter().enumerate() {
            let offset = pool.lod_data[range.first as usize + index].index_offset as usize;
            let indices: &[u8] = bytemuck::cast_slice(&lod.indices);
            assert_eq!(&bytes[offset..offset + indices.len()], indices);
        }
    }
    gpu.retire(Resource::Buffer(destination))?;
    gpu.collect()?;
    Ok(())
}

#[test]
fn reordered_publications_copy_resident_sources_and_rejected_candidates_leave_them_live()
-> Result<()> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/reused-geometry");
    let mut gpu = VulkanDevice::new_raster(&[], cache)?;
    let original = fixture();
    let pool = GeometryGpu::new(&mut gpu, &original, false)?;
    let usage = gpu.descriptor_usage();
    let mut scene = fixture();
    scene.geometry.reverse();
    for vertex in &mut scene.geometry[0].vertices {
        vertex.position[0] += 0.25;
    }
    for bound in &mut scene.geometry[0].bounds {
        bound.x += 0.25;
    }
    scene.geometry[0].revision += 1;
    let mut added = fixture().geometry.remove(0);
    added.key = [7; 32];
    scene.geometry.push(added);
    let candidate = GeometryGpu::new_validated(&mut gpu, scene.validated()?, false, Some(&pool))?;
    assert_eq!(candidate.stats().reused_geometries, 2);
    assert!(candidate.stats().reused_bytes > 0);
    assert_eq!(
        candidate.stats().uploaded_bytes + candidate.stats().reused_bytes,
        candidate.stats().bytes
    );
    assert!(candidate.structures.is_empty());
    verify_static(&mut gpu, &candidate, &scene)?;
    candidate.destroy(&mut gpu)?;
    assert_eq!(gpu.descriptor_usage(), usage);
    verify_static(&mut gpu, &pool, &original)?;

    // Removing the skin and reusing only the rigid source needs no host static
    // staging; the table remains fresh and future publication can still recover.
    let mut rigid = fixture();
    rigid.geometry.truncate(1);
    let candidate = GeometryGpu::new_validated(&mut gpu, rigid.validated()?, false, Some(&pool))?;
    assert_eq!(candidate.stats().reused_geometries, 1);
    assert_eq!(
        candidate.stats().uploaded_bytes,
        (2 * std::mem::size_of::<Row>()) as u64
    );
    verify_static(&mut gpu, &candidate, &rigid)?;
    candidate.destroy(&mut gpu)?;

    // A changed rigid revision must upload its current attributes rather than
    // copy the old bytes, even when the producer retains its source identity.
    rigid.geometry[0].revision += 1;
    rigid.geometry[0].vertices[0].uv = [0.25, 0.75];
    let candidate = GeometryGpu::new_validated(&mut gpu, rigid.validated()?, false, Some(&pool))?;
    assert_eq!(candidate.stats().reused_geometries, 0);
    assert_eq!(candidate.stats().reused_bytes, 0);
    verify_static(&mut gpu, &candidate, &rigid)?;
    candidate.destroy(&mut gpu)?;
    rigid.geometry[0].vertices[0].uv[0] = f32::NAN;
    assert!(GeometryGpu::new(&mut gpu, &rigid, false).is_err());
    assert_eq!(gpu.descriptor_usage(), usage);
    pool.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
