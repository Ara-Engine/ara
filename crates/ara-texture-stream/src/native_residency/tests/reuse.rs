use super::*;
use ara_render::instanced::Textures;

#[test]
fn replacement_reuses_images_across_reordered_ids_and_keeps_rejected_content_live() -> Result<()> {
    let cache = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-texture-stream/native-reuse");
    let mut gpu = GpuDevice::new_raster(&[], cache)?;
    let mut old = bank(1024, 1024);
    let a = old.register([1; 32], source([4, 4], 1, 64))?;
    let b = old.register([2; 32], source([4, 4], 1, 128))?;
    let color = Arc::new(NativeSource::from_cooked(
        write_ktx2(4, 4, TextureFormat::Bc7Srgb, &[vec![0; 16]])?.into(),
    )?);
    let srgb = old.register([4; 32], Arc::clone(&color))?;
    old.initialize(&mut gpu)?;
    let a_image = old.entries[a.0 as usize].resident.unwrap();
    let b_image = old.entries[b.0 as usize].resident.unwrap();
    let srgb_image = old.entries[srgb.0 as usize].resident.unwrap().image;
    let baseline = gpu.descriptor_usage();
    let mut empty = bank(1024, 1024);
    assert_eq!(empty.initialize_reusing(&mut gpu, &old)?.textures, 0);
    assert!(empty.binding().is_none());
    empty.publish_reuse(&mut old)?;
    empty.destroy(&mut gpu)?;
    assert_eq!(gpu.descriptor_usage(), baseline);
    for _ in 0..8 {
        let mut rejected = bank(1024, 1024);
        rejected.register([2; 32], source([4, 4], 1, 128))?;
        rejected.register([1; 32], source([4, 4], 1, 64))?;
        assert_eq!(
            rejected.initialize_reusing(&mut gpu, &old)?.uploaded_bytes,
            0
        );
        assert_eq!(rejected.entries[0].resident.unwrap().image, b_image.image);
        assert_eq!(rejected.entries[1].resident.unwrap().image, a_image.image);
        // A later geometry/pipeline rejection destroys only the candidate table.
        rejected.destroy(&mut gpu)?;
        assert_eq!(gpu.descriptor_usage(), baseline);
        assert_eq!(blocks(&mut gpu, a_image), [64, 64, 0, 0, 0, 0, 0, 0]);
    }
    let mut failed = bank(1, 1);
    failed.register([1; 32], source([4, 4], 1, 64))?;
    assert!(failed.initialize_reusing(&mut gpu, &old).is_err());
    failed.destroy(&mut gpu)?;
    assert_eq!(gpu.descriptor_usage(), baseline);

    let mut next = bank(1024, 1024);
    next.register([2; 32], source([4, 4], 1, 128))?;
    next.register([3; 32], source([4, 4], 1, 192))?;
    next.register_view([4; 32], color, NativeView::Raw)?;
    let stats = next.initialize_reusing(&mut gpu, &old)?;
    assert_eq!(
        stats.uploaded_bytes, 24,
        "only a changed digest/view uploads"
    );
    assert_eq!(next.entries[0].resident.unwrap().image, b_image.image);
    assert_ne!(next.entries[2].resident.unwrap().image, srgb_image);
    next.publish_reuse(&mut old)?;
    assert!(old.entries[b.0 as usize].resident.is_none());
    assert!(old.entries[a.0 as usize].resident.is_some());
    old.destroy(&mut gpu)?;
    assert_eq!(
        blocks(&mut gpu, next.entries[0].resident.unwrap()),
        [128, 128, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        blocks(&mut gpu, next.entries[1].resident.unwrap()),
        [192, 192, 0, 0, 0, 0, 0, 0]
    );
    next.destroy(&mut gpu)?;
    gpu.assert_clean()?;
    Ok(())
}
