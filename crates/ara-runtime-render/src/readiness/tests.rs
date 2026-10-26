use super::*;
use crate::*;

#[test]
fn readiness_waits_for_probe_delivery_and_convergence_but_allows_motion() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/runtime-render/readiness");
    let mut presenter = Presenter::new([64, 48], Vec::new(), root);
    presenter.set_quality(QualityChoice::Preset(QualityPreset::Low));
    assert!(!presenter.readiness().is_ready());
    let mut frame = RenderFrame::default();
    frame.draws.push(Draw {
        entity: 1,
        address: "e_box".into(),
        label: "box".into(),
        geometry: Geometry::Shape {
            shape: ara_runtime::components::MeshShape::Box,
            size: [1.; 3],
        },
        world: glam::Mat4::IDENTITY,
        color: [0.5, 0.2, 0.1, 1.],
        roughness: 1.,
        metallic: 0.,
        emissive: [0.; 3],
        emissive_factor: None,
        material: None,
        skin: None,
    });
    presenter.present(&frame)?;
    assert!(!presenter.readiness().is_ready());
    presenter.wait_probe_gi()?;
    assert!(!presenter.readiness().gi_volume_ready);
    for _ in 0..8 {
        presenter.present(&frame)?;
    }
    assert!(
        !presenter.readiness().gi_volume_ready,
        "completed relighting must still finish its arrival fade"
    );
    for _ in 0..32 {
        presenter.present(&frame)?;
    }
    assert!(
        presenter.readiness().is_ready(),
        "{:?}",
        presenter.readiness()
    );
    frame.draws[0].world = glam::Mat4::from_translation(Vec3::new(0.01, 0., 0.));
    presenter.present(&frame)?;
    presenter.wait_probe_gi()?;
    for i in 0..32 {
        frame.draws[0].world =
            glam::Mat4::from_translation(Vec3::new(0.02 + i as f32 * 0.001, 0., 0.));
        presenter.present(&frame)?;
    }
    assert!(
        presenter.raster.as_ref().expect("raster").needs_frame(),
        "moving actors still need frames"
    );
    assert!(
        presenter.readiness().is_ready(),
        "motion must not starve measurement: {:?}",
        presenter.readiness()
    );
    frame.draws[0].world = glam::Mat4::ZERO;
    presenter.present(&frame)?;
    assert!(presenter.readiness().failed);
    assert!(
        !presenter.readiness().is_ready(),
        "last-good fallback is not a ready frame"
    );
    presenter
        .renderer()
        .expect("renderer")
        .gpu()
        .assert_clean()?;
    Ok(())
}
