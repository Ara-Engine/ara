//! Independent format/quality checks. Wire this module with `#[cfg(test)]`.
//! The ignored Vulkan probe consumes bc4.json/bc5.json from ARA_BC45_COOKED;
//! run it through `ara-dev gpu -- <test-binary> --ignored gpu_matches_reference`.

use crate::bc45_reference::{encode_bc4, encode_bc5};

// This decoder uses floating rational interpolation directly from the format,
// independently of the encoder's integer palette and selector packing.
fn decode(block: [u32; 2]) -> [f64; 16] {
    let bits = u64::from(block[0]) | (u64::from(block[1]) << 32);
    let a = (bits & 255) as f64;
    let b = ((bits >> 8) & 255) as f64;
    std::array::from_fn(|i| {
        let index = ((bits >> (16 + i * 3)) & 7) as u32;
        match index {
            0 => a,
            1 => b,
            6 if a <= b => 0.0,
            7 if a <= b => 255.0,
            _ => {
                let d = if a > b { 7 } else { 5 };
                ((d + 1 - index) as f64 * a + (index - 1) as f64 * b) / d as f64
            }
        }
    })
}

fn square_error(pixels: &[u32; 16], block: [u32; 2], channel: u32) -> f64 {
    pixels
        .iter()
        .zip(decode(block))
        .map(|(&word, decoded)| (((word >> (channel * 8)) & 255) as f64 - decoded).powi(2))
        .sum()
}

fn fixtures() -> Vec<[u32; 16]> {
    let mut result = Vec::new();
    for value in 0..=255 {
        result.push([value | ((255 - value) << 8); 16]);
    }
    result.push(std::array::from_fn(|i| if i & 1 == 0 { 0 } else { 65535 }));
    result.push(std::array::from_fn(|i| {
        let values = [0, 33, 77, 121, 165, 209, 255, 121];
        values[i % 8] | (values[(i + 3) % 8] << 8)
    }));
    for values in [
        [224, 0, 192, 160, 128, 96, 64, 32],
        [40, 190, 70, 100, 130, 160, 0, 255],
    ] {
        result.push(std::array::from_fn(|i| {
            values[i % 8] | (values[(i + 3) % 8] << 8)
        }));
    }
    let mut state = 0x1234_5678u32;
    for block in 0..96 {
        result.push(std::array::from_fn(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            match block % 4 {
                0 => state,
                1 => (state & 15) | (((state >> 8) & 15) << 8),
                2 => (240 + (state & 15)) | ((240 + ((state >> 8) & 15)) << 8),
                _ => {
                    let r = (block * 7 + i * 3).min(255) as u32;
                    r | ((255 - r) << 8) | (state & 0xffff_0000)
                }
            }
        }));
    }
    result
}

#[test]
fn decoder_covers_both_orders_and_every_selector_including_word_boundary() {
    for (a, b) in [(210u32, 35u32), (35, 210), (93, 93)] {
        let mut bits = u64::from(a | (b << 8));
        for i in 0..16 {
            bits |= (i as u64 % 8) << (16 + i * 3);
        }
        let values = decode([bits as u32, (bits >> 32) as u32]);
        assert_eq!(values[0], a as f64);
        assert_eq!(values[1], b as f64);
        assert_eq!(values[..8], values[8..]);
        if a > b {
            assert_eq!(values[2], 185.0);
            assert_eq!(values[7], 60.0);
        } else {
            assert_eq!(values[6], 0.0);
            assert_eq!(values[7], 255.0);
            assert_eq!(values[2], (4 * a + b) as f64 / 5.0);
        }
    }
}

#[test]
fn every_constant_channel_round_trips_exactly_at_all_presets() {
    for value in 0..=255 {
        let pixels = [value | ((255 - value) << 8); 16];
        for preset in 0..3 {
            let encoded = encode_bc5(&pixels, preset);
            assert_eq!(decode([encoded[0], encoded[1]]), [value as f64; 16]);
            assert_eq!(decode([encoded[2], encoded[3]]), [(255 - value) as f64; 16]);
        }
    }
}

#[test]
fn exact_palettes_select_both_endpoint_orders_and_pack_all_indices() {
    for (a, b, palette) in [
        (224u32, 0, [224, 0, 192, 160, 128, 96, 64, 32]),
        (40, 190, [40, 190, 70, 100, 130, 160, 0, 255]),
    ] {
        let pixels = std::array::from_fn(|i| palette[i % 8]);
        let mut bits = u64::from(a | (b << 8));
        for i in 0..16 {
            bits |= (i as u64 % 8) << (16 + i * 3);
        }
        for preset in 0..3 {
            let encoded = encode_bc4(&pixels, preset);
            assert_eq!(encoded, [bits as u32, (bits >> 32) as u32]);
            assert_eq!(decode(encoded), pixels.map(f64::from));
        }
    }
}

#[test]
fn presets_retain_error_quality_and_channels_are_independent() {
    for pixels in fixtures() {
        let mut previous = [f64::INFINITY; 2];
        for preset in 0..3 {
            let encoded = encode_bc5(&pixels, preset);
            let red = [encoded[0], encoded[1]];
            let green = [encoded[2], encoded[3]];
            assert_eq!(red, encode_bc4(&pixels, preset));
            assert_eq!(green, encode_bc4(&pixels.map(|v| v >> 8), preset));
            for (c, block) in [red, green].into_iter().enumerate() {
                let error = square_error(&pixels, block, c as u32);
                assert!(error <= previous[c] + 1e-8, "preset={preset}, channel={c}");
                previous[c] = error;
            }
            assert_eq!(
                encoded,
                encode_bc5(&pixels.map(|v| v ^ 0xabcd_0000), preset)
            );
        }
        assert_eq!(encode_bc5(&pixels, 2), encode_bc5(&pixels, u32::MAX));
    }
}

fn normal(r: f64, g: f64) -> [f64; 3] {
    let x = r / 127.5 - 1.0;
    let y = g / 127.5 - 1.0;
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    let length = (x * x + y * y + z * z).sqrt();
    [x / length, y / length, z / length]
}

#[test]
fn smooth_masks_and_normal_maps_keep_psnr_and_angular_quality() {
    for preset in 0..3 {
        let mut square = 0.0;
        let mut angle_square = 0.0;
        let mut worst_angle = 0.0f64;
        for by in 0..16 {
            for bx in 0..16 {
                let pixels = std::array::from_fn(|i| {
                    let x = (bx * 4 + i % 4) as u32;
                    let y = (by * 4 + i / 4) as u32;
                    let r = 32 + x * 3;
                    let g = 32 + y * 3;
                    r | (g << 8)
                });
                let encoded = encode_bc5(&pixels, preset);
                let red = decode([encoded[0], encoded[1]]);
                let green = decode([encoded[2], encoded[3]]);
                square += square_error(&pixels, [encoded[0], encoded[1]], 0);
                square += square_error(&pixels, [encoded[2], encoded[3]], 1);
                for i in 0..16 {
                    let source = normal((pixels[i] & 255) as f64, ((pixels[i] >> 8) & 255) as f64);
                    let decoded = normal(red[i], green[i]);
                    let dot: f64 = source.into_iter().zip(decoded).map(|(a, b)| a * b).sum();
                    let angle = dot.clamp(-1.0, 1.0).acos().to_degrees();
                    angle_square += angle * angle;
                    worst_angle = worst_angle.max(angle);
                }
            }
        }
        let psnr = 10.0 * (255.0f64.powi(2) * 8192.0 / square).log10();
        let angular_rms = (angle_square / 4096.0).sqrt();
        eprintln!(
            "BC5 preset={preset}: {psnr:.2} dB, angle RMS={angular_rms:.3}, max={worst_angle:.3} deg"
        );
        assert!(psnr > 48.0);
        assert!(angular_rms < 1.0);
        assert!(worst_angle < 8.0);
    }
}

fn edge(
    resource: ara_gpu::Resource,
    before: ara_gpu::Access,
    after: ara_gpu::Access,
) -> ara_gpu::Barrier {
    ara_gpu::Barrier {
        resource,
        before,
        after,
        handoff: None,
    }
}

#[test]
#[ignore = "requires Vulkan, cooked bc4/bc5 manifests, and ara-dev gpu lock"]
fn gpu_matches_reference() -> Result<(), Box<dyn std::error::Error>> {
    use ara_gpu::{
        Access, Device, GpuDevice, Queue, Resource, RootConstants, cooked::CookedPipeline, vk,
        vulkan::Command,
    };
    let cooked =
        std::path::PathBuf::from(std::env::var_os("ARA_BC45_COOKED").ok_or("set ARA_BC45_COOKED")?);
    let profile = std::path::PathBuf::from(
        std::env::var_os("ARA_BC45_PROFILE").ok_or("set a fresh ARA_BC45_PROFILE")?,
    );
    let mut gpu = GpuDevice::new_raster(&[], profile)?;
    let cases = fixtures();
    let input_bytes: Vec<u8> = cases
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let input = gpu.create_buffer(
        input_bytes.len() as u64,
        vk::BufferUsageFlags::STORAGE_BUFFER,
        true,
    )?;
    gpu.upload(input, &input_bytes)?;
    let input_slot = gpu.storage_buffer(input)?.index;
    for (name, width) in [("bc4", 2), ("bc5", 4)] {
        let manifest: CookedPipeline =
            serde_json::from_slice(&std::fs::read(cooked.join(format!("{name}.json")))?)?;
        let pipeline = gpu.create_pipeline(&manifest, &cooked)?;
        let bytes = (cases.len() + 2) * width * 4;
        let output = gpu.create_buffer(bytes as u64, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        let output_slot = gpu.storage_buffer(output)?.index;
        for preset in 0..3 {
            gpu.upload(output, &vec![0xa5; bytes])?;
            let mut commands = gpu.begin(Queue::Graphics);
            commands.barriers([
                edge(
                    Resource::Buffer(input),
                    if name == "bc4" && preset == 0 {
                        Access::HostWrite
                    } else {
                        Access::ShaderRead
                    },
                    Access::ShaderRead,
                ),
                edge(Resource::Buffer(output), Access::HostWrite, Access::Storage),
            ]);
            commands.push(Command::Pipeline(pipeline));
            commands.push(Command::Constants(RootConstants {
                words: [
                    input_slot,
                    output_slot,
                    cases.len() as u32,
                    preset,
                    0,
                    0,
                    0,
                    0,
                ],
            }));
            // Extra groups must return uniformly before reading or writing.
            commands.push(Command::Dispatch([cases.len() as u32 + 2, 1, 1]));
            commands.barriers([edge(
                Resource::Buffer(output),
                Access::Storage,
                Access::HostRead,
            )]);
            let point = gpu.submit(commands.finish(), &[])?;
            gpu.resolve(point)?;
            let actual = gpu.read(output)?;
            for (i, pixels) in cases.iter().enumerate() {
                let expected: Vec<u8> = if width == 2 {
                    encode_bc4(pixels, preset)
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect()
                } else {
                    encode_bc5(pixels, preset)
                        .into_iter()
                        .flat_map(u32::to_le_bytes)
                        .collect()
                };
                assert_eq!(
                    &actual[i * width * 4..(i + 1) * width * 4],
                    expected,
                    "{name}, preset={preset}, block={i}"
                );
            }
            assert!(actual[cases.len() * width * 4..].iter().all(|&v| v == 0xa5));
            eprintln!(
                "{name} preset={preset}: {} exact GPU/reference blocks, guards intact",
                cases.len()
            );
        }
        // A zero-length cohort must leave the entire destination untouched.
        gpu.upload(output, &vec![0xa5; bytes])?;
        let mut commands = gpu.begin(Queue::Graphics);
        commands.barriers([edge(
            Resource::Buffer(output),
            Access::HostWrite,
            Access::Storage,
        )]);
        commands.push(Command::Pipeline(pipeline));
        commands.push(Command::Constants(RootConstants {
            words: [input_slot, output_slot, 0, 2, 0, 0, 0, 0],
        }));
        commands.push(Command::Dispatch([2, 1, 1]));
        commands.barriers([edge(
            Resource::Buffer(output),
            Access::Storage,
            Access::HostRead,
        )]);
        let point = gpu.submit(commands.finish(), &[])?;
        gpu.resolve(point)?;
        assert!(gpu.read(output)?.iter().all(|&v| v == 0xa5));
        gpu.retire(Resource::Buffer(output))?;
        gpu.retire_pipeline(pipeline)?;
    }
    gpu.retire(Resource::Buffer(input))?;
    gpu.collect()?;
    gpu.assert_clean()?;
    Ok(())
}
