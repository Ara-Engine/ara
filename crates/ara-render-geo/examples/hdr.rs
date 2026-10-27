//! Manual display ramp: --mode=sdr|hdr10|scrgb --paper-white=203 --frames=120.
use anyhow::{Context, Result};
use ara_color::{OutputMode, OutputSettings};
use ara_gpu::{Access, Barrier, Device, GpuDevice, Queue, Resource, commands::Command, vk};
use ara_platform::{Present, Window};
use ara_render_geo::display::DisplayOutput;
use std::path::Path;
fn main() -> Result<()> {
    let mut settings = OutputSettings::default();
    let mut frames = 120;
    for arg in std::env::args().skip(1) {
        if let Some(mode) = arg.strip_prefix("--mode=") {
            settings.mode = match mode {
                "sdr" => OutputMode::Sdr,
                "hdr10" => OutputMode::Hdr10,
                "scrgb" => OutputMode::ScRgb,
                _ => anyhow::bail!("expected sdr, hdr10 or scrgb"),
            };
        }
        if let Some(value) = arg.strip_prefix("--paper-white=") {
            settings.paper_white = value.parse()?;
        }
        if let Some(value) = arg.strip_prefix("--frames=") {
            frames = value.parse()?;
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = root.join("captures/hdr");
    std::fs::create_dir_all(&dir)?;
    let window = Window::new(
        "Ara HDR ramp | diffuse white left, highlights right",
        [1024, 384],
    )?;
    let mut gpu = GpuDevice::new(&window.extensions()?, dir.join("cache"))?;
    // SAFETY: Window/device are retained until explicit idle swapchain teardown.
    let mut present = match unsafe { Present::create_with_output(&gpu, &window, settings) } {
        Ok(p) => p,
        Err(error) => {
            ara_gpu::cooked::write_json(
                &dir.join(format!("{:?}-availability.json", settings.mode)),
                &serde_json::json!({"settings":settings,"available":false,"reason":error.to_string()}),
            )?;
            return Err(error);
        }
    };
    let size = [1024, 384];
    let mut pixels = Vec::<[f32; 4]>::new();
    for y in 0..size[1] {
        for x in 0..size[0] {
            let value = if x < 256 {
                x as f32 / 255.
            } else {
                2f32.powf((x - 256) as f32 / 767. * 6.)
            };
            let color = match y / 96 {
                0 => [value, value, value, 1.],
                1 => [value, 0., 0., 1.],
                2 => [0., value, 0., 1.],
                _ => [0., 0., value, 1.],
            };
            pixels.push(color);
        }
    }
    let input = gpu.create_image(
        size[0],
        size[1],
        vk::Format::R32G32B32A32_SFLOAT,
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
    )?;
    let stage = gpu.create_buffer(
        pixels.len() as u64 * 16,
        vk::BufferUsageFlags::TRANSFER_SRC,
        true,
    )?;
    gpu.upload(stage, bytemuck::cast_slice(&pixels))?;
    let mut r = gpu.begin(Queue::Graphics);
    r.barriers([Barrier {
        resource: Resource::Image(input),
        before: Access::Undefined,
        after: Access::CopyDestination,
        handoff: None,
    }]);
    r.push(Command::UploadImage {
        source: stage,
        destination: input,
    });
    r.barriers([Barrier {
        resource: Resource::Image(input),
        before: Access::CopyDestination,
        after: Access::ShaderRead,
        handoff: None,
    }]);
    let p = gpu.submit(r.finish(), &[])?;
    gpu.resolve(p)?;
    let mut output = DisplayOutput::new(&mut gpu, input, size, settings, &root.join("cooked"))?;
    let result = (|| -> Result<()> {
        let list = output.record(&gpu)?;
        let p = gpu.submit(list, &[])?;
        let timings = gpu.resolve(p)?;
        let encoded = output.encoded_pixels(&mut gpu)?;
        // SDR proxy shows input dynamic range compressed to one screen; it is
        // an inspection artifact, never proof of the monitor's HDR appearance.
        let proxy: Vec<u8> = pixels
            .iter()
            .flat_map(|p| {
                let c = OutputSettings::default()
                    .encode(OutputSettings::default().tone_map([p[0], p[1], p[2]]));
                [
                    (c[0] * 255.).round() as u8,
                    (c[1] * 255.).round() as u8,
                    (c[2] * 255.).round() as u8,
                    255,
                ]
            })
            .collect();
        image::save_buffer(
            dir.join("ramp-sdr-proxy.png"),
            &proxy,
            size[0],
            size[1],
            image::ColorType::Rgba8,
        )?;
        let format = present.surface_format();
        ara_gpu::cooked::write_json(
            &dir.join(format!("{:?}-availability.json", settings.mode)),
            &serde_json::json!({"settings":settings,"available":true,"format":format!("{:?}",format.format),"color_space":format!("{:?}",format.color_space),"timings":timings,"encoded_white":encoded[255],"encoded_highlight":encoded[1023],"manual_hdr_display_check":"Not established by this automated run; follow HDR.md"}),
        )?;
        for _ in 0..frames {
            if window
                .poll()?
                .into_iter()
                .any(|event| matches!(event, ara_platform::Event::Quit))
            {
                break;
            }
            // SAFETY: Output is completed CopySource and encoded for this swapchain.
            unsafe {
                present.present_encoded(&gpu, output.output(), size, settings.mode)?;
            }
        }
        gpu.assert_clean().context("native HDR ramp validation")?;
        Ok(())
    })();
    // SAFETY: All portable submissions completed; this wait also retires presentation.
    unsafe {
        gpu.wait_idle()?;
        present.retire(&gpu);
    }
    output.retire(&mut gpu)?;
    gpu.retire(Resource::Image(input))?;
    gpu.retire(Resource::Buffer(stage))?;
    gpu.collect()?;
    result
}
