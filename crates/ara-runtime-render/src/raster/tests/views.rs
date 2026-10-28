use super::*;

const CAMERA: [f32; 3] = [0., 2., 5.];
const FOV_DEGREES: f32 = 55.;

/// A deterministic capture of `mode` at 96x64 from the scene's fixed camera.
fn capture(renderer: &mut Renderer, mode: ViewMode) -> Result<ara_render::capture::Capture> {
    let request = CaptureRequest {
        camera: CameraSpec::LookAt {
            position: glam30::Vec3::from_array(CAMERA),
            target: glam30::Vec3::ZERO,
            vertical_fov: FOV_DEGREES.to_radians(),
        },
        size: [96, 64],
        mode,
        deterministic: true,
        exposure: 2e-5,
        settle_frames: 1,
        tick: 0,
        seed: 0,
        marks: None,
    };
    renderer.capture(&request)
}

fn luma(image: &image::RgbaImage, [x, y]: [u32; 2]) -> f32 {
    let p = image.get_pixel(x, y).0;
    0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])
}

#[test]
fn gi_diffuse_view_shows_indirect_diffuse_without_direct_light_or_emission() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/gi-diffuse-view")
        .join(std::process::id().to_string());
    // The sun stands behind the box, so its shadow falls towards the camera.
    let mut frame = RenderFrame::default();
    frame.sun = Some(([0., 0.6, -0.8], [100_000.; 3]));
    frame
        .draws
        .push(box_draw(1, [8., 0.2, 8.], [0., -0.1, 0.], [0.; 3]));
    frame
        .draws
        .push(box_draw(2, [1.; 3], [0., 0.5, 0.], [50_000.; 3]));
    let candidate = RasterContent::new(frame, Content::default())?;
    let mut renderer = Renderer::new_native(
        candidate.native.scene(),
        Box::new(candidate.prepared.textures.fork_sources(texture_config())?),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    assert!(renderer.capabilities().gi_diffuse);
    let mut raster = RasterView::new(root.join("probes"));
    raster.set_probe_gi(false);
    let plan = RasterPlan {
        render_size: [96, 64],
        display_size: [96, 64],
        quality: *renderer.quality(),
    };
    raster.publish_scene(&mut renderer, candidate, plan)?;
    let lit = capture(&mut renderer, ViewMode::Lit)?.image;
    let gi = capture(&mut renderer, ViewMode::GiDiffuse)?.image;
    std::fs::create_dir_all(&root)?;
    gi.save(root.join("gi_diffuse.png"))?;
    lit.save(root.join("lit.png"))?;
    let (shadowed, sunlit, emitter) = ([48, 39], [70, 39], [48, 28]);
    for (pixel, entity) in [(shadowed, 1), (sunlit, 1), (emitter, 2)] {
        assert_eq!(renderer.entity_at(pixel)?, Some(entity), "{pixel:?}");
    }
    // Direct sun separates the box's shadow from the open floor in Lit; the diffuse
    // indirect light barely does (sky and bounce reach both).
    let contrast = |image: &image::RgbaImage| luma(image, sunlit) / luma(image, shadowed).max(1.);
    let (lit_contrast, gi_contrast) = (contrast(&lit), contrast(&gi));
    assert!(
        lit_contrast > 2. && gi_contrast < 1.5,
        "sunlit / shadowed floor: lit {lit_contrast}, gi {gi_contrast}"
    );
    assert!(
        luma(&gi, shadowed) > 0.,
        "the shadowed floor still receives sky light"
    );
    // The emitter loses its emission and shows only the light it receives.
    let (lit_emitter, gi_emitter) = (luma(&lit, emitter), luma(&gi, emitter));
    assert!(
        gi_emitter < 0.5 * lit_emitter,
        "emitter: lit {lit_emitter}, gi {gi_emitter}"
    );
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}

/// Fragments the raster draws for the ray through render pixel `[x, y]` of a 96x64
/// view: two per box it crosses (closed boxes, no face culling), counted on the CPU.
fn ray_fragments(boxes: &[([f32; 3], [f32; 3])], [x, y]: [u32; 2]) -> u32 {
    use glam30::Vec3;
    let origin = Vec3::from_array(CAMERA);
    let forward = (-origin).normalize();
    let right = forward.cross(Vec3::Y).normalize();
    let up = right.cross(forward);
    let tan = (FOV_DEGREES.to_radians() * 0.5).tan();
    let ndc = [(x as f32 + 0.5) / 48. - 1., 1. - (y as f32 + 0.5) / 32.];
    let dir = (forward + right * (ndc[0] * tan * 1.5) + up * (ndc[1] * tan)).normalize();
    let crossed = |&(size, centre): &([f32; 3], [f32; 3])| {
        let (lo, hi) = (
            Vec3::from_array(centre) - Vec3::from_array(size) * 0.5,
            Vec3::from_array(centre) + Vec3::from_array(size) * 0.5,
        );
        let (a, b) = ((lo - origin) / dir, (hi - origin) / dir);
        let (near, far) = (a.min(b).max_element(), a.max(b).min_element());
        near < far && near > 0.
    };
    2 * boxes.iter().filter(|b| crossed(b)).count() as u32
}

#[test]
fn overdraw_view_counts_every_fragment_before_depth_rejection() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/overdraw-view")
        .join(std::process::id().to_string());
    // A floor slab, a box and a second box partly behind it.
    let boxes = [
        ([8., 0.2, 8.], [0., -0.1, 0.]),
        ([1., 1., 1.], [0., 0.5, 0.]),
        ([1., 1., 1.], [0.5, 0.5, -1.5]),
    ];
    let mut frame = RenderFrame::default();
    for (i, (size, centre)) in boxes.iter().enumerate() {
        frame
            .draws
            .push(box_draw(i as u64 + 1, *size, *centre, [0.; 3]));
    }
    let candidate = RasterContent::new(frame, Content::default())?;
    let mut renderer = Renderer::new_native(
        candidate.native.scene(),
        Box::new(candidate.prepared.textures.fork_sources(texture_config())?),
        [96, 64],
        &[],
        root.join("gpu"),
    )?;
    assert!(renderer.capabilities().overdraw);
    let mut raster = RasterView::new(root.join("probes"));
    raster.set_probe_gi(false);
    let plan = RasterPlan {
        render_size: [96, 64],
        display_size: [96, 64],
        quality: *renderer.quality(),
    };
    raster.publish_scene(&mut renderer, candidate, plan)?;
    let capture = capture(&mut renderer, ViewMode::Overdraw)?;
    std::fs::create_dir_all(&root)?;
    capture.image.save(root.join("overdraw.png"))?;
    let stats = capture.metadata.overdraw.expect("overdraw statistics");
    let expected = ara_render::capture::OverdrawStats::of(
        (0..64)
            .flat_map(|y| (0..96).map(move |x| [x, y]))
            .map(|p| ray_fragments(&boxes, p)),
    );
    // Rasterization and the CPU's pixel-centre rays agree but for silhouette pixels.
    assert_eq!(
        (stats.max, stats.pixels),
        (expected.max, expected.pixels),
        "{stats:?}"
    );
    for (band, (gpu, cpu)) in stats.bands.iter().zip(expected.bands).enumerate() {
        assert!(
            gpu.abs_diff(cpu) <= 60,
            "band {band}: gpu {stats:?}, cpu {expected:?}"
        );
    }
    assert_eq!(stats.max, 6, "rays through both boxes and the floor");
    // The fixed legend: nothing is black, two fragments (the floor alone) green.
    assert_eq!(capture.image.get_pixel(2, 2).0, [0, 0, 0, 255]);
    let floor = [6, 62];
    assert_eq!(ray_fragments(&boxes, floor), 2);
    assert_eq!(
        capture.image.get_pixel(floor[0], floor[1]).0,
        [0, 255, 0, 255]
    );
    raster.destroy(&mut renderer)?;
    renderer.gpu().assert_clean()?;
    Ok(())
}
