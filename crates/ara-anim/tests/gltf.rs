//! Generated glTF fixture exercises the real gltf skin/channel readers without downloads.
#![allow(clippy::unwrap_used)]
use ara_anim::{
    data::*,
    glam::{Mat4, Quat},
    *,
};
use gltf::animation::util::ReadOutputs;

#[test]
fn gltf_skin_animation_to_cooked_handoff() {
    let mut binary = Vec::new();
    let mut append = |values: &[f32]| {
        for v in values {
            binary.extend(v.to_le_bytes());
        }
    };
    append(&[0.0, 0.5, 1.0]);
    append(&[0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let rotations: Vec<f32> = [0.0, 0.5, 1.0]
        .iter()
        .flat_map(|r| Quat::from_rotation_z(*r).to_array())
        .collect();
    append(&rotations);
    append(&Mat4::IDENTITY.to_cols_array());
    append(&Mat4::from_translation(glam::Vec3::new(0.0, -1.0, 0.0)).to_cols_array());
    let json = serde_json::json!({
        "asset":{"version":"2.0"}, "buffers":[{"byteLength":binary.len()}],
        "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12},{"buffer":0,"byteOffset":12,"byteLength":36},{"buffer":0,"byteOffset":48,"byteLength":48},{"buffer":0,"byteOffset":96,"byteLength":128}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"SCALAR","min":[0],"max":[1]}, {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}, {"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"}, {"bufferView":3,"componentType":5126,"count":2,"type":"MAT4"}],
        "nodes":[{"name":"Root","children":[1]},{"name":"Hand","translation":[0,1,0]}],
        "skins":[{"joints":[0,1],"skeleton":0,"inverseBindMatrices":3}],
        "animations":[{"samplers":[{"input":0,"output":1,"interpolation":"LINEAR"},{"input":0,"output":2,"interpolation":"LINEAR"}],"channels":[{"sampler":0,"target":{"node":0,"path":"translation"}},{"sampler":1,"target":{"node":1,"path":"rotation"}}]}]
    });
    let gltf = gltf::Gltf::from_slice(&serde_json::to_vec(&json).unwrap()).unwrap();
    let skin = gltf.skins().next().unwrap();
    let joints: Vec<_> = skin.joints().collect();
    let mut data = SkeletonData::default();
    for joint in &joints {
        let parent = joints
            .iter()
            .position(|p| p.children().any(|c| c.index() == joint.index()))
            .map(|i| i as u32);
        let (translation, rotation, scale) = joint.transform().decomposed();
        data.joints.push(Joint {
            id: format!("gltf_{}", joint.index()),
            name: joint.name().unwrap_or("").into(),
            parent,
            bind: AnimTransform {
                translation,
                rotation,
                scale,
            },
            ..Default::default()
        });
    }
    let imported_inverse: Vec<_> = skin
        .reader(|_| Some(binary.as_slice()))
        .read_inverse_bind_matrices()
        .unwrap()
        .collect();
    data.inverse_bind = imported_inverse
        .iter()
        .map(|m| AnimMatrix {
            x: m[0],
            y: m[1],
            z: m[2],
            w: m[3],
        })
        .collect();
    let skeleton = Skeleton::cook(data).unwrap();
    let mut clip = ClipData {
        duration: 48000,
        tracks: vec![JointTrack::default(); joints.len()],
        ..Default::default()
    };
    for channel in gltf.animations().next().unwrap().channels() {
        let joint = joints
            .iter()
            .position(|j| j.index() == channel.target().node().index())
            .unwrap();
        let reader = channel.reader(|_| Some(binary.as_slice()));
        let ticks: Vec<_> = reader
            .read_inputs()
            .unwrap()
            .map(|t| (t * 48000.0).round() as u32)
            .collect();
        match reader.read_outputs().unwrap() {
            ReadOutputs::Translations(values) => {
                clip.tracks[joint].translations = ticks
                    .into_iter()
                    .zip(values)
                    .map(|(tick, value)| VectorKey { tick, value })
                    .collect()
            }
            ReadOutputs::Rotations(values) => {
                clip.tracks[joint].rotations = ticks
                    .into_iter()
                    .zip(values.into_f32())
                    .map(|(tick, value)| RotationKey { tick, value })
                    .collect()
            }
            _ => panic!("unexpected generated channel"),
        }
    }
    let (cooked, report) =
        CookedClip::cook(&clip, &skeleton, CompressionSettings::default()).unwrap();
    assert!(report.rotation_error < 0.0005);
    let mut sampler = Sampler::new(&cooked);
    let mut pose = Pose::new(2);
    sampler
        .sample(&cooked, &skeleton, 24000, &mut pose, &mut [])
        .unwrap();
    assert!((pose.translations()[0].x - 0.5).abs() < 0.0001);
    assert!((pose.rotations()[1].to_axis_angle().1 - 0.5).abs() < 0.0005);
    let mut model = vec![Mat4::IDENTITY; 2];
    let mut palette = model.clone();
    pose::model_matrices(&skeleton, skeleton.bind_pose(), &mut model).unwrap();
    pose::skinning_palette(&skeleton, &model, &mut palette).unwrap();
    for (i, inverse) in imported_inverse.iter().enumerate() {
        assert!((model[i] * Mat4::from_cols_array_2d(inverse)).abs_diff_eq(palette[i], 1e-6));
    }
}
