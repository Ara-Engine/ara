//! Independent expectation math only; native acceptance comes from the SDL driver.
use super::proof::box_weight;

#[test]
fn axis_aligned_weight_oracle_covers_inside_blend_outside_and_corner_distance() {
    let half = [5., 2.5, 5.];
    assert_eq!(box_weight([0., 0., 0.], half, 2., 0.8), 0.8);
    assert!((box_weight([0., 0., 6.], half, 2., 0.8) - 0.4).abs() < 0.0001);
    assert_eq!(box_weight([0., 0., 8.], half, 2., 0.8), 0.);
    // The diagonal distance is sqrt(1^2+1^2), rather than either face distance.
    let expected = 0.8 * (1. - 2f32.sqrt() / 2.);
    assert!((box_weight([6., 0., 6.], half, 2., 0.8) - expected).abs() < 0.0001);
}
#[test]
fn oracle_distinguishes_actual_outside_blend_and_inside_camera_samples() {
    let weights = [10., 6.5, 4.5].map(|z| box_weight([0., 0., z], [5., 2.5, 5.], 3.5, 0.64));
    assert_eq!(weights[0], 0.);
    assert!(weights[1] > 0. && weights[1] < weights[2]);
    assert_eq!(weights[2], 0.64);
}
