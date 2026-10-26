use super::*;

#[test]
fn a_preset_without_terrain_detail_fills_the_gbuffer_through_its_own_pso() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/gbuffer-top")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let scene = shadows::scene();
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [128, 96],
        &[],
        root.join("gpu"),
    )?;
    let pipelines = renderer.native.as_ref().unwrap().pipelines;
    assert_ne!(pipelines.gbuffer_top, pipelines.gbuffer);
    assert_eq!(pipelines.gbuffer_for(0.), pipelines.gbuffer_top);
    assert_eq!(pipelines.gbuffer_for(40.), pipelines.gbuffer);
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: Vec3::new(0., 0., 3.),
            target: Vec3::ZERO,
            vertical_fov: 0.8,
        },
        size: [128, 96],
        mode: ViewMode::Albedo,
        deterministic: true,
        exposure: 1.,
        settle_frames: 1,
        tick: 0,
        seed: 7,
        marks: None,
    };
    let detailed = renderer.capture(&request)?;
    // Without terrain in view the two G-buffer PSOs agree to the byte.
    let quality = crate::look::Quality {
        terrain_detail: 0.,
        ..*renderer.quality()
    };
    renderer.set_quality(quality)?;
    let top = renderer.capture(&request)?;
    assert!(
        top.image.as_raw().iter().any(|&c| c > 0),
        "the casters are drawn"
    );
    assert_eq!(top.image.as_raw(), detailed.image.as_raw());
    renderer.gpu().assert_clean()?;
    Ok(())
}
