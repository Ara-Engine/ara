use super::*;

// Mode 6: constant RGBA endpoints with zero interpolation indices. Two adjacent
// blocks expose the different ordering of transfer-function decoding/filtering.
fn constant_bc7(value: u8) -> [u8; 16] {
    let mut bits = 1u128 << 6;
    let mut offset = 7;
    for channel in [value, value, value, 254] {
        for _ in 0..2 {
            bits |= u128::from(channel / 2) << offset;
            offset += 7;
        }
    }
    bits.to_le_bytes()
}

fn color_source() -> Arc<NativeSource> {
    Arc::new(
        NativeSource::from_cooked(
            write_ktx2(
                8,
                4,
                TextureFormat::Bc7Srgb,
                &[[constant_bc7(32), constant_bc7(224)].concat()],
            )
            .unwrap()
            .into(),
        )
        .unwrap(),
    )
}

#[test]
fn effective_view_policy_participates_in_residency_identity() {
    let mut set = bank(100, 100);
    let color = color_source();
    let srgb = set.register([1; 32], Arc::clone(&color)).unwrap();
    let raw = set
        .register_view([1; 32], Arc::clone(&color), NativeView::Raw)
        .unwrap();
    assert_ne!(srgb, raw);
    assert_eq!(
        raw,
        set.register_view([1; 32], color, NativeView::Raw).unwrap()
    );
    let data = source([4, 4], 1, 64);
    let cooked = set.register([2; 32], Arc::clone(&data)).unwrap();
    assert_eq!(
        cooked,
        set.register_view([2; 32], data, NativeView::Raw).unwrap()
    );
    assert_eq!(set.entries.len(), 3);
    let fork = set
        .fork_sources(NativeConfig {
            resident_bytes: 1000,
            upload_bytes: 1000,
        })
        .unwrap();
    assert!(fork.binding().is_none());
    assert_eq!(
        fork.content_keys().collect::<Vec<_>>(),
        set.content_keys().collect::<Vec<_>>()
    );
    assert!(Arc::ptr_eq(&fork.entries[0].source, &set.entries[0].source));
    assert_eq!(fork.stats().resident_bytes, 0);
}

#[test]
fn native_color_views_preserve_gltf_and_graph_filtering_contracts() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut gpu = VulkanDevice::new(
        &[],
        root.join("../../target/ara-texture-stream/native-views"),
    )
    .unwrap();
    let cooked = root.join("cooked");
    let manifest =
        serde_json::from_slice(&std::fs::read(cooked.join("texture_native_probe.json")).unwrap())
            .unwrap();
    let pipeline = gpu.create_pipeline(&manifest, &cooked).unwrap();
    let mut set = bank(1000, 1000);
    let color = color_source();
    set.register([1; 32], Arc::clone(&color)).unwrap();
    set.register_view([1; 32], color, NativeView::Raw).unwrap();
    let grazing = [255, 255, 0, 0, 0, 0, 0, 0, 128, 128, 0, 0, 0, 0, 0, 0];
    set.register(
        [2; 32],
        Arc::new(
            NativeSource::from_cooked(
                write_ktx2(4, 4, TextureFormat::Bc5, &[grazing.to_vec()])
                    .unwrap()
                    .into(),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    set.initialize(&mut gpu).unwrap();
    let linear = sample(&mut gpu, &mut set, pipeline, 0..1, 0.0, Probe::Color);
    let raw = sample(&mut gpu, &mut set, pipeline, 1..2, 0.0, Probe::Color);
    let graph = sample(&mut gpu, &mut set, pipeline, 1..2, 0.0, Probe::DecodeColor);
    let decode = |v: f32| ((v + 0.055) / 1.055).powf(2.4);
    for (actual, expected) in [
        (
            linear[0],
            (decode(32.0 / 255.0) + decode(224.0 / 255.0)) * 0.5,
        ),
        (raw[0], 128.0 / 255.0),
        (graph[0], decode(128.0 / 255.0)),
    ] {
        assert!((actual - expected).abs() < 0.002, "{actual} != {expected}");
    }
    for pixel in [&linear, &raw, &graph] {
        assert!((pixel[3] - 254.0 / 255.0).abs() < 1e-5);
    }
    assert_eq!(
        sample(&mut gpu, &mut set, pipeline, 2..3, 0.0, Probe::Normal(0.0)),
        [0.0, 0.0, 1.0, 1.0]
    );
    set.destroy(&mut gpu).unwrap();
    gpu.retire_pipeline(pipeline).unwrap();
    gpu.collect().unwrap();
    gpu.assert_clean().unwrap();
}
