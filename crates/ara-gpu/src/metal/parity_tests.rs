//! Tests for the VulkanDevice-parity surface: deferred completion, preparation
//! workers and windowless CAMetalLayer presentation.
use super::*;
use crate::commands::{Attachment, Command};
use crate::vulkan::{
    BufferPreparation, ImagePreparation, PipelinePreparation, ResourcePreparation,
};

fn device() -> MetalDevice {
    MetalDevice::new_raster(&[], std::env::temp_dir()).expect("Metal 4 device")
}

#[test]
fn deferred_scope_orders_in_flight_constant_writes_and_polls_without_waiting() {
    let mut gpu = device();
    let constants = gpu
        .create_buffer(16, vk::BufferUsageFlags::TRANSFER_SRC, true)
        .expect("constants");
    let readback = gpu.create_readback_buffer(32).expect("readback");
    gpu.begin_deferred_completion().expect("scope");
    assert!(gpu.deferring_completion());
    gpu.upload_queued(constants, bytemuck::cast_slice(&[1u32, 2, 3, 4]))
        .expect("idle write maps directly");
    let copy = |gpu: &mut MetalDevice, offset| {
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::CopyBufferRange {
            source: constants,
            source_offset: 0,
            destination: readback,
            destination_offset: offset,
            size: 16,
        });
        gpu.submit(rec.finish(), &[]).expect("copy");
    };
    copy(&mut gpu, 0);
    // `upload_queued` takes this path while the copy is in flight; calling it directly
    // keeps the test independent of how fast the GPU finishes the first copy.
    gpu.upload_ordered(constants, 0, bytemuck::cast_slice(&[5u32, 6, 7, 8]))
        .expect("ordered write");
    copy(&mut gpu, 16);
    assert_eq!(gpu.deferred_submission_count(), 3);
    let mut frame = gpu.end_deferred_completion().expect("end");
    assert!(
        gpu.begin_deferred_completion().is_err(),
        "unpolled frame blocks a new scope"
    );
    gpu.wait_completion(&mut frame).expect("wait");
    assert!(frame.is_resolved());
    let values: Vec<u32> = bytemuck::cast_slice(&gpu.read(readback).expect("read")).to_vec();
    assert_eq!(values, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert!(
        gpu.poll_completion(&mut frame).is_err(),
        "resolved frames are spent"
    );
    gpu.begin_deferred_completion().expect("next scope");
    let mut empty = gpu.end_deferred_completion().expect("end");
    assert!(gpu.poll_completion(&mut empty).expect("poll").is_some());
}

#[test]
fn resource_preparation_admits_two_requests_and_adopts_initialized_buffers() {
    let mut gpu = device();
    let request = |bytes: Vec<u8>| ResourcePreparation {
        buffers: vec![BufferPreparation {
            size: 8,
            usage: vk::BufferUsageFlags::STORAGE_BUFFER,
            host: true,
            data: crate::vulkan::InitialData::deferred(move || Ok(bytes)),
        }],
        images: vec![ImagePreparation {
            size: [4, 4],
            format: vk::Format::R8G8B8A8_UNORM,
            mip_levels: 3,
            usage: vk::ImageUsageFlags::SAMPLED,
        }],
    };
    let first = gpu
        .prepare_resources(|| request(vec![1; 8]))
        .expect("prepare")
        .expect("admitted");
    let second = gpu
        .prepare_resources(|| request(vec![2; 8]))
        .expect("prepare")
        .expect("admitted");
    assert!(
        gpu.prepare_resources(|| unreachable!("a third request is not admitted"))
            .expect("prepare")
            .is_none()
    );
    gpu.cancel_prepared_resources(second).expect("cancel");
    assert!(matches!(
        gpu.poll_prepared_resources(second),
        Err(GpuError::StaleHandle)
    ));
    let prepared = loop {
        if let Some(prepared) = gpu.poll_prepared_resources(first).expect("poll") {
            break prepared;
        }
        std::thread::yield_now();
    };
    assert_eq!(gpu.read(prepared.buffers[0]).expect("read"), vec![1; 8]);
    assert_eq!(
        gpu.image_extent(prepared.images[0]).expect("extent"),
        [4, 4]
    );
    gpu.storage_buffer(prepared.buffers[0])
        .expect("adopted buffers bind");
}

#[test]
fn pipeline_preparation_reports_missing_metal_artifacts_as_a_contract_error() {
    let mut gpu = device();
    let cooked = cooked::CookedPipeline {
        version: 1,
        key: "probe".into(),
        binding_layout_hash: cooked::binding_layout_hash(),
        compiler: "probe".into(),
        stages: vec![cooked::CookedStage {
            source_entry: "main".into(),
            spirv_entry: "main".into(),
            stage: cooked::Stage::Compute,
            spirv: "probe.spv".into(),
            dxil: "probe.dxil".into(),
            spirv_hash: String::new(),
            dxil_hash: String::new(),
            metal: None,
        }],
        state: cooked::RenderState::default(),
    };
    let id = gpu
        .prepare_pipelines(|| {
            vec![PipelinePreparation {
                cooked,
                artifacts: std::env::temp_dir(),
            }]
        })
        .expect("prepare")
        .expect("admitted");
    let result = loop {
        match gpu.poll_prepared_pipelines(id) {
            Ok(None) => std::thread::yield_now(),
            other => break other,
        }
    };
    assert!(
        matches!(result, Err(GpuError::Contract(message)) if message.contains("no Metal artifacts"))
    );
}

#[test]
fn windowless_presenter_copies_a_completed_frame_into_the_drawable() {
    let mut gpu = device();
    let size = [32, 16];
    let mut presenter = present::Presenter::new(&gpu, size).expect("presenter");
    assert_eq!(
        presenter.surface_format().format,
        vk::Format::B8G8R8A8_UNORM
    );
    let frame = gpu
        .create_image(
            size[0],
            size[1],
            vk::Format::B8G8R8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("frame");
    let mut rec = gpu.begin(Queue::Graphics);
    rec.push(Command::BeginRendering {
        colors: vec![Attachment {
            image: frame,
            clear: Some([0.25, 0.5, 1.0, 1.0]),
        }],
        depth: None,
        size,
    });
    rec.push(Command::EndRendering);
    gpu.submit(rec.finish(), &[]).expect("render");
    let readback = gpu
        .create_readback_buffer(u64::from(size[0] * size[1] * 4))
        .expect("readback");
    presenter
        .present(&mut gpu, frame, Some(readback))
        .expect("present");
    let pixels: Vec<u32> = bytemuck::cast_slice(&gpu.read(readback).expect("read")).to_vec();
    // BGRA bytes of (0.25, 0.5, 1.0, 1.0): b=255, g=128, r=64, a=255.
    assert!(
        pixels.iter().all(|&p| p == 0xff40_80ff),
        "{:08x}",
        pixels[0]
    );
    let wrong = gpu
        .create_image(
            8,
            8,
            vk::Format::B8G8R8A8_UNORM,
            vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("wrong size");
    assert!(matches!(
        presenter.display(&mut gpu, wrong),
        Err(GpuError::Contract(_))
    ));
}
