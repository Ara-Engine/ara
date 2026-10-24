use super::*;
use ara_asset::{
    CachedImport, Cancellation, CookPreset, ImportContext, ImportInput, TargetPlatform,
};
use ara_render::{Renderer, capture::ViewMode, scene::CameraSpec};
use ara_render_fidelity::{
    environment::FrameEnvironment,
    presets::{Capabilities, Plan, QualityChoice, QualityPreset, RenderOptions, RenderPath},
};
use ara_runtime::{animation::Library, assets::MemoryAssets};

#[test]
fn raster_adopts_staged_model_publications_without_unwritten_frames_or_descriptor_growth()
-> Result<()> {
    let registry = ara_importers::registry()?;
    let importer = registry.get("ara.gltf").context("fixture importer")?;
    let mut panel: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../../../ara-importers/tests/fixtures/material-extensions.gltf"
    ))?;
    // This test observes publication, not transmission or back-face policy.
    panel["materials"][0]["doubleSided"] = serde_json::json!(true);
    panel["materials"][0]["extensions"]["KHR_materials_transmission"]["transmissionFactor"] =
        serde_json::json!(0);
    let input = ImportInput {
        preset: CookPreset::Fast,
        thread_budget: 1,
        source_path: "panel.gltf".into(),
        source: serde_json::to_vec(&panel)?,
        importer: importer.id().into(),
        version: importer.version().into(),
        settings: serde_json::json!({"virtualGeometry":false}),
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    };
    let generation = CachedImport {
        key: input.key()?,
        output: registry.import(
            &input,
            &mut ImportContext {
                metrics: None,
                cancellation: &Cancellation::default(),
                progress: &mut |_| {},
            },
        )?,
    };
    let mut source = MemoryAssets::default();
    let workspace = ara_doc::Workspace::default();
    let schema = ara_runtime::schema(&[])?;
    let entities = (0..3).map(|i| {
        (format!("e_panel{i}"), ara_doc::prefab::ResolvedEntity {
            record: serde_json::from_value(serde_json::json!({"MeshRenderer":{"model":format!("@panel{i}.gltf#as_panel{i}")}})).unwrap(),
            parent: None, order: None,
            source: (ara_doc::AssetId::parse("sc_pending").unwrap(), ara_doc::RecordId::parse(&format!("e_panel{i}")).unwrap()),
        })
    }).collect();
    let transforms = (0..3)
        .map(|i| {
            (
                format!("e_panel{i}"),
                glam30::Mat4::from_translation(glam30::Vec3::new(i as f32 * 2., 0., 0.)),
            )
        })
        .collect();
    let mut library = Library::default();
    let mut raster = crate::RasterView::new("target/unused-probe-cache");
    raster.set_probe_gi(false);
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/renderer-unification/staged-publication")
        .join(std::process::id().to_string());
    let mut renderer = None;
    for stage in 0..=3 {
        if stage > 0 {
            source
                .0
                .insert(format!("as_panel{}", stage - 1), generation.clone());
        }
        let frame = document_frame(
            &entities,
            &transforms,
            &workspace,
            &mut library,
            &source,
            None,
            &Default::default(),
        )?;
        let models = frame
            .draws
            .iter()
            .filter(|draw| matches!(draw.geometry, Geometry::Model { .. }))
            .count();
        assert_eq!(models > 0, stage > 0);
        assert_eq!(library.models.get("as_panel2").is_some(), stage == 3);
        if renderer.is_none() {
            let prepared = prepare(&frame, Content::default(), config())?;
            renderer = Some(Renderer::new_native(
                &prepared.scene,
                Box::new(prepared.textures),
                [128, 96],
                &[],
                root.join("gpu"),
            )?);
        }
        let renderer = renderer.as_mut().context("test renderer")?;
        let plan = Plan::resolve(
            QualityChoice::Preset(QualityPreset::Low),
            &Capabilities::of(renderer.gpu()),
            [128, 96],
            RenderOptions::default(),
        )?;
        let RenderPath::Raster(plan) = plan.path else {
            panic!("raster default")
        };
        let look = FrameEnvironment::resolve(&frame.look, frame.environment.exposure);
        let camera = CameraSpec::LookAt {
            position: glam30::Vec3::new(2.5, 5., 5.),
            target: glam30::Vec3::new(2.5, 0., 0.5),
            vertical_fov: 1.,
        };
        let mut baseline = None;
        for _ in 0..24 {
            raster.prepare(renderer, &frame, plan, look, || {
                Content::resolve_available(&frame, &workspace, &schema, &library, &source)
            })?;
            raster.render(renderer, &frame, &camera, ViewMode::Lit)?;
            assert!(renderer.presentation_ready());
            let usage = renderer.gpu().descriptor_usage();
            assert_eq!(
                *baseline.get_or_insert(usage),
                usage,
                "pending retries must have flat descriptor usage in stage {stage}"
            );
            renderer.gpu().assert_clean()?;
        }
        eprintln!("publication stage {stage}: {baseline:?}");
        renderer
            .capture_completed(&ara_render::capture::CaptureRequest {
                camera,
                size: [128, 96],
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: 1.,
                settle_frames: 1,
                tick: 0,
                seed: 0,
                marks: None,
            })?
            .write(&root.join(format!("stage-{stage}.png")))?;
        if stage == 3 {
            // Extraction accepts the pose before a later quality failure. Retrying
            // must still publish that pose, rather than forgetting its dirty row.
            let mut changed = frame;
            changed.draws[0].world.w_axis.x += 1.;
            let mut invalid = plan;
            invalid.quality.shadows.resolution = 0;
            assert!(
                raster
                    .prepare(renderer, &changed, invalid, look, || Ok(Content::default()))
                    .is_err()
            );
            let recovered =
                raster.prepare(renderer, &changed, plan, look, || Ok(Content::default()))?;
            assert!(
                recovered.rebuilt,
                "rejected preparation must not consume pending pose edits"
            );
            raster.render(renderer, &changed, &camera, ViewMode::Lit)?;
            renderer.gpu().assert_clean()?;
        }
    }
    raster.destroy(renderer.as_mut().context("test renderer")?)?;
    Ok(())
}

#[test]
fn pending_assets_do_not_hide_bad_published_generations() {
    let mut source = MemoryAssets::default();
    assert!(
        when_published(&source, |source| {
            let mut models = ara_runtime::assets::Models::default();
            Ok(models.load(source, "as_pending")?)
        })
        .unwrap()
        .is_none()
    );
    source.0.insert(
        "as_pending".into(),
        ara_asset::CachedImport {
            key: AssetDigest::of(b"invalid cook"),
            output: ara_asset::ImportOutput {
                artifacts: Vec::new(),
            },
        },
    );
    assert!(
        when_published(&source, |source| {
            let mut models = ara_runtime::assets::Models::default();
            Ok(models.load(source, "as_pending")?)
        })
        .is_err()
    );
}
