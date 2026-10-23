#![allow(clippy::unwrap_used)]
mod common;
use ara_anim::{
    data::*,
    glam::{Mat4, Quat, Vec3, Vec3A},
    *,
};

#[test]
fn linear_layer_additive_and_palette_match_transform_algebra() {
    let s = common::skeleton(2);
    let mut a = s.bind_pose().clone();
    let mut b = a.clone();
    b.set_joint(
        1,
        Transform {
            translation: Vec3A::new(2.0, 0.2, 0.0),
            rotation: Quat::from_rotation_z(1.0),
            scale: Vec3A::splat(2.0),
        },
    )
    .unwrap();
    a.blend(&b, 0.5, Some(&[0.0, 1.0])).unwrap();
    common::near(a.translations()[1].x, 1.0, 1e-6);
    common::near(a.rotations()[1].to_axis_angle().1, 0.5, 1e-6);
    assert_eq!(a.scales()[1], Vec3A::splat(1.5));
    assert_eq!(a.joint(0), s.bind_pose().joint(0));
    let mut additive = s.bind_pose().clone();
    additive.add(&b, s.bind_pose(), 0.5, None).unwrap();
    assert_eq!(additive, a);
    let before = a.clone();
    assert!(a.blend(&b, 1.0, Some(&[f32::NAN, 0.5])).is_err());
    assert_eq!(a, before);
    let mut model = vec![Mat4::IDENTITY; 2];
    let mut palette = model.clone();
    pose::model_matrices(&s, s.bind_pose(), &mut model).unwrap();
    pose::skinning_palette(&s, &model, &mut palette).unwrap();
    assert!(palette.iter().all(|m| m.abs_diff_eq(Mat4::IDENTITY, 1e-6)));
    pose::model_matrices(&s, &b, &mut model).unwrap();
    pose::skinning_palette(&s, &model, &mut palette).unwrap();
    assert!(!palette[1].abs_diff_eq(Mat4::IDENTITY, 0.1));
}

#[test]
fn analytic_ik_reaches_target_and_look_at_respects_limit() {
    let root = Mat4::IDENTITY;
    let middle = Mat4::from_translation(Vec3::X);
    let tip = Mat4::from_translation(Vec3::X * 2.0);
    let target = Vec3::new(1.0, 1.0, 0.0);
    let (a, b) = ik::two_bone(root, middle, tip, target, Vec3::Y, 1.0).unwrap();
    let actual = a * Vec3::X + b * Vec3::X;
    assert!(actual.distance(target) < 1e-5, "{actual:?}");
    assert_eq!(
        ik::two_bone(root, middle, tip, target, Vec3::Y, 0.0).unwrap(),
        (Quat::IDENTITY, Quat::IDENTITY)
    );
    let looked = ik::look_at(Quat::IDENTITY, Vec3::X, Vec3::Y, 0.4, 1.0).unwrap();
    common::near(looked.to_axis_angle().1, 0.4, 1e-6);
    assert!(ik::look_at(Quat::IDENTITY, Vec3::ZERO, Vec3::Y, 1.0, 1.0).is_err());
    let s = Skeleton::cook(SkeletonData {
        joints: vec![
            Joint {
                id: "root".into(),
                ..Default::default()
            },
            Joint {
                id: "middle".into(),
                parent: Some(0),
                bind: AnimTransform {
                    translation: [1.0, 0.0, 0.0],
                    ..Default::default()
                },
                ..Default::default()
            },
            Joint {
                id: "tip".into(),
                parent: Some(1),
                bind: AnimTransform {
                    translation: [1.0, 0.0, 0.0],
                    ..Default::default()
                },
                ..Default::default()
            },
        ],
        motion_root: 0,
        ..Default::default()
    })
    .unwrap();
    let mut p = s.bind_pose().clone();
    let mut model = vec![Mat4::IDENTITY; 3];
    pose::model_matrices(&s, &p, &mut model).unwrap();
    ik::apply_model_rotation(&s, &mut p, &model, 0, a).unwrap();
    pose::model_matrices(&s, &p, &mut model).unwrap();
    ik::apply_model_rotation(&s, &mut p, &model, 1, b).unwrap();
    pose::model_matrices(&s, &p, &mut model).unwrap();
    assert!(model[2].w_axis.truncate().distance(target) < 1e-5);
}

#[test]
fn skeleton_rejects_cycles_duplicates_and_nonrigid_motion_root_parent() {
    let mut data = common::skeleton(3).data().clone();
    data.joints[1].parent = Some(2);
    assert!(Skeleton::cook(data).is_err());
    let mut data = common::skeleton(3).data().clone();
    data.joints[1].id = data.joints[0].id.clone();
    assert!(Skeleton::cook(data).is_err());
    let mut data = common::skeleton(3).data().clone();
    data.motion_root = 1;
    assert!(Skeleton::cook(data).is_err());
}
