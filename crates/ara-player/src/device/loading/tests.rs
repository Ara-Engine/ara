use super::*;

#[test]
fn loading_and_game_share_native_lifetime_but_not_allocations() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/loading-shared-lifetime");
    let (mut loading, game, _) = Loading::for_game(&[], &root.join("first"))?;
    assert_eq!(loading.native_device(), game.context().device.handle());
    assert_eq!(game.descriptor_usage().sampled_images, 0);
    drop(game);
    loading.draw(0.5)?;
    assert_eq!(loading.read_pixels()?, loading.pixels);
    loading.gpu.assert_clean()?;
    drop(loading);

    let (mut loading, mut game, _) = Loading::for_game(&[], &root.join("second"))?;
    loading.draw(0.5)?;
    drop(loading);
    let buffer = game.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
    game.upload(buffer, &[7; 16])?;
    assert_eq!(game.read(buffer)?, vec![7; 16]);
    game.assert_clean()?;
    Ok(())
}

#[test]
fn shaderless_loading_progress_uploads_exactly_without_scene_or_descriptor_allocations()
-> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/loading-shaderless");
    let mut loading = Loading::new(&[], &root)?;
    let before = loading.gpu.descriptor_usage();
    assert_eq!(
        before.sampled_images
            + before.storage_images
            + before.storage_buffers
            + before.acceleration_structures,
        0
    );
    assert!(!loading.gpu.info().ray_tracing);
    loading.draw(0.2)?;
    assert_eq!(loading.read_pixels()?, loading.pixels);
    let initial_blue = loading
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[2] > 100)
        .count();
    for frame in 0..8 {
        loading.draw(0.2 + frame as f32 * 0.08)?;
        assert_eq!(loading.read_pixels()?, loading.pixels);
        assert_eq!(loading.gpu.descriptor_usage(), before);
    }
    let blue = loading
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[2] > 100)
        .count();
    assert!(blue > initial_blue * 2, "real progress advances the bar");
    loading.capture(&root.join("progress.png"), [1280, 720])?;
    loading.gpu.assert_clean()?;
    Ok(())
}
