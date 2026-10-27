use super::*;

#[test]
fn wireframe_modes_draw_triangle_edges_over_their_image() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/wireframe")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    let mut scene = shadows::scene();
    scene.sun_direction = Vec3::new(0.3, 0.5, 1.).normalize();
    scene.sun_lux = Vec3::splat(3.);
    let mut renderer = Renderer::new_native(
        &scene,
        Box::new(EmptyTextures),
        [160, 120],
        &[],
        root.join("gpu"),
    )?;
    let capture = |renderer: &mut Renderer, mode: ViewMode| -> Result<image::RgbaImage> {
        let image = renderer
            .capture(&CaptureRequest {
                camera: CameraSpec::LookAt {
                    position: Vec3::new(0., 0., 3.),
                    target: Vec3::ZERO,
                    vertical_fov: 0.8,
                },
                size: [160, 120],
                mode,
                deterministic: true,
                exposure: 1.,
                settle_frames: 2,
                tick: 0,
                seed: 7,
                marks: None,
            })?
            .image;
        image.save(root.join(format!("{}.png", mode.name())))?;
        Ok(image)
    };
    let lit = capture(&mut renderer, ViewMode::Lit)?;
    let ran = |r: &Renderer, pass: &str| r.timings().iter().any(|t| t.pass == pass);
    assert!(!ran(&renderer, "Wire.Draw"));
    // Over the lit image: the same picture plus amber edges.
    let overlay = capture(&mut renderer, ViewMode::LitWireframe)?;
    assert!(ran(&renderer, "Wire.Draw") && ran(&renderer, "Wire.Composite"));
    let amber = |p: &image::Rgba<u8>| p[0] > 200 && u16::from(p[0]) > u16::from(p[2]) + 80;
    let edges = overlay.pixels().filter(|p| amber(p)).count();
    assert!(edges > 40, "edges drawn over the lit image ({edges})");
    assert_eq!(lit.pixels().filter(|p| amber(p)).count(), 0);
    let changed = lit
        .pixels()
        .zip(overlay.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        changed < (160 * 120) / 4,
        "only edges change ({changed} pixels)"
    );
    // The wireframe view: a dark background (0.025 linear, sRGB 44) with bright edges.
    let wire = capture(&mut renderer, ViewMode::Wireframe)?;
    let bright = wire
        .pixels()
        .filter(|p| p[0].max(p[1]).max(p[2]) > 90)
        .count();
    let dark = wire
        .pixels()
        .filter(|p| p[0].max(p[1]).max(p[2]) < 70)
        .count();
    assert!(
        bright > 40 && dark > (160 * 120) / 2,
        "{bright} bright, {dark} dark"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
