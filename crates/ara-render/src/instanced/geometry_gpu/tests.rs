use super::*;
use crate::instanced::{Geometry, Lod, Material};
use ara_gpu::{
    Barrier, Queue, RootConstants,
    acceleration::{AccelerationDesc, AccelerationInstance},
    commands::Command,
    cooked::CookedPipeline,
    vk,
};
use glam::Vec3;

fn fixture() -> Scene {
    let geometry = [0.0, 2.0]
        .into_iter()
        .enumerate()
        .map(|(index, x)| Geometry {
            key: [index as u8; 32],
            vertices: [[x - 0.5, -0.5, 0.0], [x + 0.5, -0.5, 0.0], [x, 0.5, 0.0]]
                .map(|position| Vertex {
                    position,
                    normal: [0.0, 0.0, 1.0],
                    uv: [0.0; 2],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv1: [0.0; 2],
                    terrain_weights: 0,
                    reserved: 0,
                })
                .to_vec(),
            lods: vec![
                Lod {
                    indices: vec![0, 1, 2],
                    error: 0.0,
                },
                Lod {
                    indices: vec![2, 1, 0],
                    error: 0.1,
                },
            ],
            bounds: [Vec3::new(x - 0.5, -0.5, 0.0), Vec3::new(x + 0.5, 0.5, 0.0)],
            deforming: index == 1,
            revision: 1,
        })
        .collect();
    Scene {
        geometry,
        instances: Vec::new(),
        materials: vec![Material::default()],
        textures: 0,
        lights: Vec::new(),
        sun_direction: Vec3::Y,
        sun_lux: Vec3::ONE,
        environment: vec![[1.0; 4]],
        environment_size: [1, 1],
    }
}

#[test]
fn pooling_preserves_local_lod_indices_and_separates_skin_vertices() {
    let scene = fixture();
    scene.validate().unwrap();
    assert!(pack::Packed::new(&scene, 100).is_err());
    let packed = pack::Packed::new(&scene, 4096).unwrap();
    assert_eq!(packed.static_bytes.len(), 3 * 64 + 12 * 4);
    assert_eq!(packed.dynamic.len(), 3);
    assert_eq!(packed.lods.len(), 4);
    assert_eq!((packed.ranges[1].first, packed.ranges[1].count), (2, 2));
    assert_eq!(packed.lods[0].vertex_offset, packed.lods[1].vertex_offset);
    assert_eq!(packed.lods[2].vertex_offset, 0);
    assert_eq!(packed.lods[2].index_offset, 3 * 64 + 6 * 4);
}

#[test]
fn uploaded_pool_builds_shared_lods_and_traces_transformed_instances() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(&[], root.join("../../target/ara-render/native-geometry"))?;
    let scene = fixture();
    let raster = GeometryGpu::new(&mut gpu, &scene, false)?;
    assert!(raster.table_index().is_some() && raster.blas(0).is_none());
    assert_eq!(raster.resources().count(), 5);
    raster.destroy(&mut gpu)?;
    if !gpu.context().info.ray_tracing {
        return Ok(());
    }
    let mut pool = GeometryGpu::new(&mut gpu, &scene, true)?;
    assert_eq!(pool.stats().geometries, 2);
    assert_eq!(pool.stats().blas, 4);
    assert_eq!(
        pool.lods(1).map(|range| (range.first, range.count)),
        Some((2, 2))
    );
    let a = pool
        .blas(0)
        .ok_or_else(|| anyhow::anyhow!("static BLAS missing"))?;
    let b = pool
        .blas(2)
        .ok_or_else(|| anyhow::anyhow!("skin BLAS missing"))?;
    let instances = [(a, 11, 0.0), (b, 23, 0.0), (a, 47, 4.0)]
        .into_iter()
        .map(|(geometry, custom_index, x)| AccelerationInstance {
            geometry,
            custom_index,
            mask: 255,
            transform: [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        })
        .collect();
    let tlas = gpu.create_acceleration_structure(&AccelerationDesc::Instances(instances))?;
    let output = gpu.create_buffer(12, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    let output_slot = gpu.storage_buffer(output)?.index;
    let tlas_slot = gpu.bind_acceleration_structure(tlas)?.index;
    let cooked = root.join("../ara-gpu/tests/cooked");
    let manifest: CookedPipeline =
        serde_json::from_slice(&std::fs::read(cooked.join("triangle_ranges.json"))?)?;
    let pipeline = gpu.create_pipeline(&manifest, &cooked)?;
    let mut commands = gpu.begin(Queue::Graphics);
    commands.push(Command::BuildAccelerationStructure(tlas));
    commands.barriers([
        Barrier {
            resource: Resource::AccelerationStructure(tlas),
            before: Access::AsBuild,
            after: Access::AsRead,
            handoff: None,
        },
        Barrier {
            resource: Resource::Buffer(output),
            before: Access::Undefined,
            after: Access::Storage,
            handoff: None,
        },
    ]);
    commands.push(Command::Pipeline(pipeline));
    commands.push(Command::Constants(RootConstants {
        words: [tlas_slot, output_slot, 0, 0, 0, 0, 0, 0],
    }));
    commands.push(Command::Dispatch([1, 1, 1]));
    commands.barriers([Barrier {
        resource: Resource::Buffer(output),
        before: Access::Storage,
        after: Access::HostRead,
        handoff: None,
    }]);
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    assert_eq!(
        gpu.read(output)?,
        bytemuck::cast_slice::<_, u8>(&[11u32, 23, 47])
    );
    eprintln!(
        "native pool: {}B, upload {:.6}ms, BLAS {:.6}ms",
        pool.stats().bytes,
        pool.stats().upload_ms,
        pool.stats().build_ms
    );
    gpu.retire_pipeline(pipeline)?;
    gpu.retire(Resource::AccelerationStructure(tlas))?;
    gpu.retire(Resource::Buffer(output))?;
    let mut scene = scene;
    for vertex in &mut scene.geometry[1].vertices {
        vertex.position[0] += 0.2;
    }
    for bound in &mut scene.geometry[1].bounds {
        bound.x += 0.2;
    }
    scene.geometry[1].revision += 1;
    let table = pool.table_index();
    let pose = pool
        .prepare_pose(&mut gpu, &scene)?
        .expect("changed skin stages a candidate");
    assert!(
        pool.prepare_pose(&mut gpu, &scene).is_err(),
        "a second outstanding pose must not overwrite its candidate"
    );
    assert_eq!(pool.table_index(), table, "preparation must not publish");
    pool.cancel_pose(&mut gpu, pose)?;
    assert_eq!(
        pool.blas(2),
        Some(b),
        "cancel must retain the published BLAS"
    );
    assert_eq!(pool.table_index(), table);
    let pose = pool
        .prepare_pose(&mut gpu, &scene)?
        .expect("canceled pose may be retried");
    assert_eq!(pose.geometries, 1);
    assert_eq!(pose.blas, 2, "both LODs of the changed skin need rebuilds");
    pool.commit_pose(&mut gpu, pose)?;
    assert_eq!(pool.blas(0), Some(a), "skin update must retain static BLAS");
    assert_ne!(pool.blas(2), Some(b));
    assert_ne!(pool.table_index(), table);
    pool.presented();
    assert!(pool.prepare_pose(&mut gpu, &scene)?.is_none());
    pool.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
