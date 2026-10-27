use super::*;
use ara_gpu::{Barrier, Buffer, Device, GpuDevice, Resource, vk};
use std::path::Path;

fn read_buffer(
    gpu: &mut GpuDevice,
    buffer: Buffer,
    state: ara_render::graph::State,
    size: u64,
) -> Result<Vec<u32>> {
    let staging = gpu.create_readback_buffer(size)?;
    let mut recorder = gpu.begin(state.queue);
    for (resource, before, after) in [
        (Resource::Buffer(buffer), state.access, Access::CopySource),
        (
            Resource::Buffer(staging),
            Access::Undefined,
            Access::CopyDestination,
        ),
    ] {
        recorder.push(Command::Barrier(Barrier {
            resource,
            before,
            after,
            handoff: None,
        }));
    }
    recorder.push(Command::CopyBuffer {
        source: buffer,
        destination: staging,
        size,
    });
    for (resource, before, after) in [
        (Resource::Buffer(buffer), Access::CopySource, state.access),
        (
            Resource::Buffer(staging),
            Access::CopyDestination,
            Access::HostRead,
        ),
    ] {
        recorder.push(Command::Barrier(Barrier {
            resource,
            before,
            after,
            handoff: None,
        }));
    }
    let point = gpu.submit(recorder.finish(), &[])?;
    gpu.resolve(point)?;
    let bytes = gpu.read(staging)?;
    gpu.retire(Resource::Buffer(staging))?;
    gpu.collect()?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

#[test]
fn indirect_sort_uses_the_compacted_live_prefix_and_matches_depth_order() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let effect = ara_vfx::samples::smoke();
    let mut gpu = GpuDevice::new_raster(&[], root.join("../../target/ara-vfx-gpu/live-sort"))?;
    let mut system = GpuSystem::new(&mut gpu, &effect, &root.join("cooked/smoke"), [64, 48])?;
    let view = View::default();
    let c = system.layout.configs[0];
    assert!(c.render[3] != 0 && c.atlas[3] >= 64);
    let mut observed = Vec::new();
    for tick in 1..=320 {
        system.step(&mut gpu, &view)?;
        if ![1, 2, 4, 8, 16, 32, 64, 128, 320].contains(&tick) {
            continue;
        }
        let stats = system.stats(&gpu)?;
        let count = stats.emitters[0].rendered;
        let n = if count == 0 {
            0
        } else {
            count.next_power_of_two()
        };
        let control = read_buffer(
            &mut gpu,
            system.control,
            system.resources[2].state,
            u64::from(system.layout.control_bytes),
        )?;
        let base = c.atlas[3] as usize / 4;
        assert_eq!(&control[base..base + 4], &[n.div_ceil(256), 1, 1, n]);
        let mut offset = base + 4;
        let mut k = 2;
        while k <= c.scratch[3] {
            let mut j = k / 2;
            while j > 0 {
                assert_eq!(
                    &control[offset..offset + 3],
                    &[if k <= n { n.div_ceil(256) } else { 0 }, 1, 1]
                );
                offset += 3;
                j /= 2;
            }
            k *= 2;
        }
        let particles = system.read_particles(&mut gpu)?;
        let lists = read_buffer(
            &mut gpu,
            system.lists,
            system.resources[1].state,
            u64::from(system.layout.list_bytes),
        )?;
        let start = c.lists[0] as usize / 4;
        let sorted = &lists[start..start + count as usize];
        let mut ids = sorted.to_vec();
        ids.sort_unstable();
        let expected: Vec<_> = particles[0]
            .iter()
            .enumerate()
            .filter(|(_, p)| p.age >= 0.)
            .take(count as usize)
            .map(|(i, _)| i as u32)
            .collect();
        assert_eq!(
            ids, expected,
            "sort retains exactly the selected live particles"
        );
        let key = |id: u32| {
            (particles[0][id as usize].position - system.last_frame.camera.truncate())
                .dot(system.last_frame.forward.truncate())
        };
        for pair in sorted.windows(2) {
            assert!(key(pair[0]) + 1e-5 >= key(pair[1]));
        }
        observed.push(n);
    }
    assert!(observed.contains(&0));
    assert!(observed.contains(&1));
    assert!(observed.iter().any(|&n| n > 1 && n < c.scratch[3]));
    system.retire(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}

// Apply the existing display curve only for comparison; production HDR composition
// leaves this to the renderer's look stack.
fn display_hdr(system: &mut GpuSystem, gpu: &mut GpuDevice) -> Result<Vec<u8>> {
    let mut recorder = gpu.begin(Queue::Graphics);
    recorder.push(Command::Barrier(Barrier {
        resource: Resource::Image(system.output),
        before: system.resources[9].state.access,
        after: Access::Color,
        handoff: None,
    }));
    system.tonemap(&mut recorder);
    let point = gpu.submit(recorder.finish(), &[])?;
    gpu.resolve(point)?;
    system.resources[9].state.access = Access::Color;
    system.read_output(gpu)
}

#[test]
fn direct_hdr_matches_the_background_copy_without_backdrop_or_tonemap() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let size = [256, 144];
    let effect = ara_vfx::samples::campfire();
    let mut gpu = GpuDevice::new_raster(&[], root.join("../../target/ara-vfx-gpu/direct-hdr"))?;
    let cooked = root.join("cooked/campfire");
    let mut copied = GpuSystem::new(&mut gpu, &effect, &cooked, size)?;
    let mut direct = GpuSystem::new(&mut gpu, &effect, &cooked, size)?;
    let usage = vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED;
    let color = gpu.create_image(size[0], size[1], vk::Format::R32G32B32A32_SFLOAT, usage)?;
    let depth = gpu.create_image(size[0], size[1], vk::Format::R32_SFLOAT, usage)?;
    let color_index = gpu.sampled_image(color, false)?.index;
    let depth_index = gpu.sampled_image(depth, false)?.index;
    for tick in 1..=240 {
        let mut recorder = gpu.begin(Queue::Graphics);
        for image in [color, depth] {
            recorder.push(Command::Barrier(Barrier {
                resource: Resource::Image(image),
                before: if tick == 1 {
                    Access::Undefined
                } else {
                    Access::ShaderRead
                },
                after: Access::Color,
                handoff: None,
            }));
        }
        recorder.push(Command::BeginRendering {
            colors: vec![
                Attachment {
                    image: color,
                    clear: Some([0.03, 0.06, 0.1, 0.]),
                },
                Attachment {
                    image: depth,
                    clear: Some([0.; 4]),
                },
            ],
            depth: None,
            size,
        });
        recorder.push(Command::EndRendering);
        for image in [color, depth] {
            recorder.push(Command::Barrier(Barrier {
                resource: Resource::Image(image),
                before: Access::Color,
                after: Access::ShaderRead,
                handoff: None,
            }));
        }
        let point = gpu.submit(recorder.finish(), &[])?;
        gpu.resolve(point)?;
        let state = ara_render::graph::State {
            access: Access::ShaderRead,
            queue: Queue::Graphics,
        };
        let scene = crate::SceneColor {
            image: color,
            sampled_index: color_index,
            state,
        };
        let mut view = View {
            scene_background: Some(scene),
            scene_depth: Some(crate::SceneDepth {
                image: depth,
                sampled_index: depth_index,
                state,
            }),
            ..View::default()
        };
        copied.step(&mut gpu, &view)?;
        view.scene_background = None;
        view.scene_color = Some(scene);
        direct.composite(&mut gpu, &view, true)?;
        assert!(
            direct
                .timings()
                .iter()
                .all(|p| p.pass != "Vfx.Backdrop" && p.pass != "Vfx.Tonemap")
        );
        assert_eq!(direct.hdr_output().image, color);
        if [60, 150, 240].contains(&tick) {
            let expected = copied.read_output(&mut gpu)?;
            let actual = display_hdr(&mut direct, &mut gpu)?;
            assert_eq!(actual, expected, "identical composition at tick {tick}");
            assert_eq!(
                direct.read_coverage(&mut gpu)?,
                copied.read_coverage(&mut gpu)?
            );
        }
    }
    assert!(direct.stats(&gpu)?.particles_alive > 0);
    copied.retire(&mut gpu)?;
    direct.retire(&mut gpu)?;
    for image in [color, depth] {
        gpu.retire(Resource::Image(image))?;
    }
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
