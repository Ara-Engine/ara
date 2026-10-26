use super::*;
use crate::look::{
    DepthOfField, DepthOfFieldQuality, Lens, Look, MotionBlurQuality, Quality, ToneCurve,
};

fn passes(renderer: &Renderer) -> Vec<String> {
    renderer.timings().iter().map(|t| t.pass.clone()).collect()
}

/// Mean squared difference between horizontal neighbours (gradient energy): blur
/// spreads an edge's step over more pixels, which lowers it.
fn sharpness(image: &image::RgbaImage) -> f64 {
    let (w, h) = image.dimensions();
    let mut sum = 0.;
    for y in 0..h {
        for x in 1..w {
            let (a, b) = (image.get_pixel(x, y), image.get_pixel(x - 1, y));
            sum += (0..3)
                .map(|c| f64::from(a[c].abs_diff(b[c])).powi(2))
                .sum::<f64>();
        }
    }
    sum / f64::from(w * h)
}

#[test]
fn volume_lens_effects_run_only_while_asked_and_blur_focus() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/lens")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root)?;
    // The casters face the camera: light them from its side.
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
    renderer.set_quality(Quality {
        depth_of_field: Some(DepthOfFieldQuality {
            samples: 24,
            max_radius: 0.03,
        }),
        motion_blur: Some(MotionBlurQuality {
            samples: 8,
            max_length: 24.,
        }),
        ..Quality::default()
    })?;
    let plain = Look {
        curve: ToneCurve::Filmic,
        ..Look::default()
    };
    renderer.set_look(plain)?;
    let camera = |x: f32| CameraSpec::LookAt {
        position: Vec3::new(x, 0., 3.),
        target: Vec3::ZERO,
        vertical_fov: 0.8,
    };
    // Off by default: no lens pass runs.
    for x in [0., 0.05] {
        renderer.render_view(&camera(x), 1., ViewMode::Lit)?;
    }
    assert!(
        !passes(&renderer)
            .iter()
            .any(|p| p.starts_with("DoF.") || p.starts_with("MotionBlur.")),
        "{:?}",
        passes(&renderer)
    );
    // Focus close in front of the casters, wide open, a moving camera and fringes.
    let lens = Look {
        lens: Lens {
            chromatic_aberration: 0.5,
            depth_of_field: Some(DepthOfField {
                focus_distance: 0.6,
                aperture: 1.4,
                near: 1.,
                far: 1.,
            }),
            motion_blur: 0.5,
        },
        ..plain
    };
    renderer.set_look(lens)?;
    for x in [0.1, 0.2, 0.3] {
        renderer.render_view(&camera(x), 1., ViewMode::Lit)?;
    }
    let ran = passes(&renderer);
    for pass in [
        "DoF.Prepare",
        "DoF.Gather",
        "DoF.Composite",
        "MotionBlur.Tiles",
        "MotionBlur.Neighbours",
    ] {
        assert!(ran.iter().any(|p| p == pass), "{pass} missing from {ran:?}");
    }
    let request = CaptureRequest {
        camera: camera(0.),
        size: [160, 120],
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 1.,
        settle_frames: 4,
        tick: 0,
        seed: 7,
        marks: None,
    };
    let blurred = renderer.capture(&request)?;
    renderer.set_look(plain)?;
    let sharp = renderer.capture(&request)?;
    let (blurred, sharp) = (sharpness(&blurred.image), sharpness(&sharp.image));
    assert!(sharp > 10., "the casters are drawn ({sharp})");
    assert!(
        blurred < sharp * 0.5,
        "focus at 0.6 m blurs casters 3 m away: {blurred} vs {sharp}"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[test]
fn motion_blur_smears_a_panning_view() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/motion-blur")
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
    renderer.set_quality(Quality {
        motion_blur: Some(MotionBlurQuality {
            samples: 16,
            max_length: 32.,
        }),
        ..Quality::default()
    })?;
    // About 20 display pixels of pan per frame; read the last frame of each run.
    let pan = |renderer: &mut Renderer, shutter: f32| -> Result<image::RgbaImage> {
        renderer.set_look(Look {
            curve: ToneCurve::Filmic,
            lens: Lens {
                motion_blur: shutter,
                ..Lens::default()
            },
            ..Look::default()
        })?;
        for frame in 0..6 {
            let x = frame as f32 * 0.4 - 1.;
            let camera = CameraSpec::LookAt {
                position: Vec3::new(x, 0., 3.),
                target: Vec3::new(x, 0., 0.),
                vertical_fov: 0.8,
            };
            renderer.render_view(&camera, 1., ViewMode::Lit)?;
        }
        let request = CaptureRequest {
            camera: CameraSpec::LookAt {
                position: Vec3::new(1., 0., 3.),
                target: Vec3::new(1., 0., 0.),
                vertical_fov: 0.8,
            },
            size: [160, 120],
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: 1.,
            settle_frames: 0,
            tick: 0,
            seed: 7,
            marks: None,
        };
        Ok(renderer.capture_completed(&request)?.image)
    };
    let still = pan(&mut renderer, 0.)?;
    let blurred = pan(&mut renderer, 1.)?;
    still.save(root.join("still.png"))?;
    blurred.save(root.join("blurred.png"))?;
    assert!(
        passes(&renderer).iter().any(|p| p == "MotionBlur.Tiles"),
        "{:?}",
        passes(&renderer)
    );
    let (still, blurred) = (sharpness(&still), sharpness(&blurred));
    assert!(still > 10., "the casters are drawn ({still})");
    assert!(
        blurred < still * 0.7,
        "a full shutter smears the pan: {blurred} vs {still}"
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}
