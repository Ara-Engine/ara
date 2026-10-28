//! Independent expectation math must distinguish the modes the native stories exercise.
#![allow(clippy::unwrap_used)] // Tests assert independent transform-oracle fixture results.
use super::*;

fn worlds() -> std::collections::BTreeMap<String, Mat4> {
    std::collections::BTreeMap::from([
        (
            FIRST.into(),
            Mat4::from_rotation_y(30f32.to_radians())
                * Mat4::from_translation(Vec3::new(-2., 0., 0.))
                * Mat4::from_rotation_z(30f32.to_radians()),
        ),
        (SECOND.into(), Mat4::from_translation(Vec3::new(2., 0., 0.))),
    ])
}
#[test]
fn world_and_local_expectations_differ_under_the_rotated_parent() {
    let before = worlds();
    let world = expected(Action::World, &before).unwrap();
    let local = expected(Action::Local, &before).unwrap();
    for id in SELECTED {
        let point = before[id].transform_point3(Vec3::ZERO);
        assert!((world[id].transform_point3(Vec3::ZERO) - point - Vec3::X * 1.1).length() < 0.0001);
        assert!(!close(world[id], local[id]));
    }
    let axis = before[FIRST].to_scale_rotation_translation().1 * Vec3::X;
    assert!(
        (local[SECOND].transform_point3(Vec3::ZERO)
            - before[SECOND].transform_point3(Vec3::ZERO)
            - axis * 1.1)
            .length()
            < 0.0001
    );
}
#[test]
fn centre_rotates_positions_and_individual_pivots_leave_positions_unchanged() {
    let before = worlds();
    let centre = expected(Action::Centre, &before).unwrap();
    let pivot = expected(Action::Pivot, &before).unwrap();
    let average = SELECTED
        .map(|id| before[id].transform_point3(Vec3::ZERO))
        .into_iter()
        .sum::<Vec3>()
        * 0.5;
    for id in SELECTED {
        let start = before[id].transform_point3(Vec3::ZERO);
        assert!((pivot[id].transform_point3(Vec3::ZERO) - start).length() < 0.0001);
        let desired = average + Quat::from_rotation_z(30f32.to_radians()) * (start - average);
        assert!((centre[id].transform_point3(Vec3::ZERO) - desired).length() < 0.0001);
        assert!(!close(centre[id], pivot[id]));
    }
}
#[test]
fn precision_expectation_uses_quarter_metre_increments_and_cancel_still_has_preview() {
    let before = worlds();
    let snapped = expected(Action::Precision, &before).unwrap();
    let cancelled_preview = expected(Action::Cancel, &before).unwrap();
    for id in SELECTED {
        assert!(
            (snapped[id].transform_point3(Vec3::ZERO)
                - before[id].transform_point3(Vec3::ZERO)
                - Vec3::X * 0.75)
                .length()
                < 0.0001
        );
        assert!(close(snapped[id], cancelled_preview[id]));
        assert!(!close(before[id], cancelled_preview[id]));
    }
}
