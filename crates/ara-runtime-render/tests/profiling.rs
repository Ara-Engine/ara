//! Real offscreen completion provenance; no window or simulation is created.
#![allow(clippy::unwrap_used)]
use ara_runtime::{
    components::MeshShape,
    extract::{CameraView, Draw, Geometry, RenderFrame},
};
use ara_runtime_render::{Presenter, QualityChoice};

#[test]
fn overlapped_gpu_samples_keep_the_submitting_profiler_frame() {
    let root = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/profile-gpu")
        });
    let mut frame = RenderFrame::default();
    frame.camera = Some(CameraView {
        position: [0., 2., 6.],
        forward: [0., -0.2, -1.],
        fov: 50.,
    });
    frame.draws.push(Draw {
        entity: 1,
        address: "e_box".into(),
        label: "Box".into(),
        geometry: Geometry::Shape {
            shape: MeshShape::Box,
            size: [1., 1., 1.],
        },
        world: glam34::Mat4::IDENTITY,
        color: [0.7, 0.5, 0.3, 1.],
        roughness: 0.8,
        metallic: 0.,
        emissive: [0.; 3],
        emissive_factor: None,
        material: None,
        skin: None,
    });
    let mut presenter = Presenter::new(
        [96, 64],
        Vec::new(),
        root.join(format!("gpu-correlation-{}", std::process::id())),
    );
    presenter.set_quality(QualityChoice::Preset(
        ara_runtime_render::QualityPreset::Low,
    ));
    presenter.set_probe_gi(false);
    presenter.set_frames_in_flight(true).unwrap();
    presenter.profile_frame(Some(101));
    presenter
        .present_with(&frame, || Ok(Default::default()))
        .unwrap();
    assert!(
        presenter.take_profile_timings().is_none(),
        "submitted work is not a completed sample"
    );
    presenter.profile_frame(Some(202));
    presenter
        .present_with(&frame, || Ok(Default::default()))
        .unwrap();
    let (origin, passes) = presenter
        .take_profile_timings()
        .expect("native timestamps from completed first submission");
    assert_eq!(
        origin, 101,
        "the second CPU frame must not acquire the first frame's GPU work"
    );
    assert!(!passes.is_empty());
    assert!(
        passes
            .iter()
            .all(|pass| pass.milliseconds.is_finite() && pass.milliseconds >= 0.)
    );
    presenter.drain().unwrap();
    assert_eq!(presenter.take_profile_timings().unwrap().0, 202);
    presenter.profile_frame(None);
    presenter
        .present_with(&frame, || Ok(Default::default()))
        .unwrap();
    presenter.drain().unwrap();
    assert!(
        presenter.take_profile_timings().is_none(),
        "unsampled submissions stay absent"
    );
}
