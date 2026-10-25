//! Compare cached readback with the existing allocation policy on identical bytes.
use ara_gpu::{Access, Barrier, Device, Queue, Resource, vk, vulkan::{Command, VulkanDevice}};

#[test]
#[ignore = "requires Vulkan; measurement must run through ara-dev gpu"]
fn cached_readback_preserves_bytes_and_measures_cpu_copy_cost() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-texture-gpu/readback-probe");
    std::fs::create_dir_all(&root)?;
    let cache = tempfile::tempdir_in(root)?;
    let mut gpu = VulkanDevice::new_raster(&[], cache.path())?;
    let bytes: Vec<_> = (0..4 * 1024 * 1024).map(|index| (index * 137 + index / 17) as u8).collect();
    let upload = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC, true)?;
    let ordinary = gpu.create_buffer(bytes.len() as u64, vk::BufferUsageFlags::TRANSFER_DST, true)?;
    let cached = gpu.create_readback_buffer(bytes.len() as u64)?;
    gpu.upload(upload, &bytes)?;
    let edge = |buffer, before, after| Barrier { resource: Resource::Buffer(buffer), before, after, handoff: None };
    let mut commands = gpu.begin(Queue::Compute);
    commands.push(Command::BeginPass("Readback.Probe.Fill".into()));
    commands.barriers([edge(upload, Access::HostWrite, Access::CopySource)]);
    for destination in [ordinary, cached] {
        commands.barriers([edge(destination, Access::Undefined, Access::CopyDestination)]);
        commands.push(Command::CopyBuffer { source: upload, destination, size: bytes.len() as u64 });
        commands.barriers([edge(destination, Access::CopyDestination, Access::HostRead)]);
    }
    commands.push(Command::EndPass);
    let point = gpu.submit(commands.finish(), &[])?;
    gpu.resolve(point)?;
    let mut timings = [Vec::new(), Vec::new()];
    for iteration in 0..12 {
        // Alternate ordering to avoid assigning every warm-cache advantage to one policy.
        for index in if iteration % 2 == 0 { [0, 1] } else { [1, 0] } {
            let start = std::time::Instant::now();
            let actual = gpu.read([ordinary, cached][index])?;
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(actual, bytes);
            if iteration >= 4 { timings[index].push(elapsed); }
        }
    }
    eprintln!("readback_cpu_ms={}", serde_json::json!({"bytes":bytes.len(),"ordinary":timings[0],"cached":timings[1]}));
    for buffer in [upload, ordinary, cached] { gpu.retire(Resource::Buffer(buffer))?; }
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
