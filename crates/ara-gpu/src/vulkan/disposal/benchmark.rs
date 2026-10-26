//! Opt-in paired driver-destruction measurement, never a timing assertion.
use super::*;
use crate::{Access, Barrier, Device, Queue, Resource, commands::Command, vulkan::VulkanDevice};
use std::time::Instant;

#[test]
#[ignore = "paired GPU retirement benchmark; run under ara-dev gpu"]
fn measure_completed_image_collection() -> anyhow::Result<()> {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ara-gpu/disposal-cost");
    let mut gpu = VulkanDevice::new_raster(&[], &root)?;
    let mut results = Vec::new();
    for repeat in 0..2 {
        for synchronous in [true, false] {
            let mut images = Vec::new();
            let mut commands = gpu.begin(Queue::Graphics);
            for _ in 0..64 {
                let image = gpu.create_image(
                    512,
                    512,
                    vk::Format::R8G8B8A8_UNORM,
                    vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT,
                )?;
                gpu.sampled_image(image, false)?;
                commands.barriers([Barrier {
                    resource: Resource::Image(image),
                    before: Access::Undefined,
                    after: Access::Color,
                    handoff: None,
                }]);
                commands.push(Command::BeginRendering {
                    colors: vec![crate::commands::Attachment {
                        image,
                        clear: Some([0.2, 0.4, 0.8, 1.]),
                    }],
                    depth: None,
                    size: [512; 2],
                });
                commands.push(Command::EndRendering);
                images.push(image);
            }
            let point = gpu.submit(commands.finish(), &[])?;
            gpu.resolve(point)?;
            let baseline = gpu.descriptor_usage();
            for image in images {
                gpu.retire(Resource::Image(image))?;
            }
            let start = Instant::now();
            if synchronous {
                // The original collection path, including teardown-record removal.
                for retired in gpu.retired.drain(..) {
                    let super::super::Retired::Image(image) = retired else {
                        anyhow::bail!("benchmark retires images only")
                    };
                    gpu.heap.retire_image(image.raw.view, image.heap);
                    gpu.context.detach_image(image.raw);
                    Object::Image(image.raw).destroy(&gpu.context.device);
                }
            } else {
                gpu.collect()?;
            }
            let owner_ms = start.elapsed().as_secs_f64() * 1000.;
            // Drain the worker between arms so no destruction overlaps allocation
            // or GPU work in the next sample; shutdown is outside owner_ms.
            drop(gpu.disposal.take());
            let drained_ms = start.elapsed().as_secs_f64() * 1000.;
            gpu.disposal = Some(Disposal::new(gpu.context.device.clone())?);
            assert_eq!(
                gpu.descriptor_usage().sampled_images + 64,
                baseline.sampled_images
            );
            results.push(serde_json::json!({
                "repeat": repeat,
                "synchronous": synchronous,
                "images": 64,
                "payload_bytes": 64 * 512 * 512 * 4,
                "owner_ms": owner_ms,
                "drained_ms": drained_ms,
            }));
        }
    }
    gpu.assert_clean()?;
    let report = serde_json::json!({"device": gpu.context.info, "samples": results});
    std::fs::create_dir_all(&root)?;
    let text = serde_json::to_string_pretty(&report)?;
    std::fs::write(root.join("paired.json"), &text)?;
    println!("{text}");
    Ok(())
}
