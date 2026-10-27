#![allow(clippy::unwrap_used)]
use super::*;
#[test]
fn framing_fits_every_corner_of_a_long_thin_box_at_portrait_and_wide_aspects() {
    let bounds = (Vec3::new(-100., -0.2, -2.), Vec3::new(100., 0.2, 2.));
    for size in [[400, 1400], [1600, 900], [2400, 400]] {
        let mut camera = Camera {
            yaw: 0.2,
            pitch: 0.15,
            ..Camera::default()
        };
        let yaw = camera.yaw;
        let pitch = camera.pitch;
        fit_aspect(&mut camera, bounds, size[0] as f32 / size[1] as f32);
        assert_eq!((camera.yaw, camera.pitch), (yaw, pitch));
        let view = camera.spec().resolve(size).unwrap();
        let vp = view.projection * view.view;
        for index in 0..8 {
            let point = Vec3::new(
                if index & 1 == 0 {
                    bounds.0.x
                } else {
                    bounds.1.x
                },
                if index & 2 == 0 {
                    bounds.0.y
                } else {
                    bounds.1.y
                },
                if index & 4 == 0 {
                    bounds.0.z
                } else {
                    bounds.1.z
                },
            );
            let clip = vp * point.extend(1.);
            let ndc = clip.truncate() / clip.w;
            assert!(
                clip.w > 0. && ndc.x.abs() <= 0.92 && ndc.y.abs() <= 0.92,
                "{size:?}: {ndc:?}"
            );
        }
    }
}
#[test]
fn framing_invalid_bounds_or_aspect_keeps_the_last_camera() {
    let mut camera = Camera::default();
    let original = camera;
    for aspect in [0., -1., f32::NAN, f32::INFINITY] {
        fit_aspect(&mut camera, (Vec3::ZERO, Vec3::ONE), aspect);
        assert_eq!(camera, original);
    }
    fit_aspect(&mut camera, (Vec3::ONE, Vec3::ZERO), 1.);
    assert_eq!(camera, original);
    fit_aspect(&mut camera, (Vec3::splat(f32::NAN), Vec3::ONE), 1.);
    assert_eq!(camera, original);
}
