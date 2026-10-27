use super::*;
use ara_asset::{
    CachedImport, Cancellation, CookPreset, ImportContext, ImportInput, TargetPlatform,
};
use ara_render::{Renderer, capture::ViewMode, scene::CameraSpec};
use ara_render_fidelity::presets::{
    Capabilities, Plan, QualityChoice, QualityPreset, RenderOptions, RenderPath,
};
use ara_runtime::{
    animation::Library,
    assets::{AssetSource, MemoryAssets},
};
use std::collections::BTreeSet;

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
    let mut placeholders = crate::ModelPlaceholders::default();
    let models = (0..3)
        .map(|i| (format!("e_panel{i}"), format!("as_panel{i}")))
        .collect();
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
        let mut frame = document_frame(
            &entities,
            &transforms,
            &workspace,
            &mut library,
            &source,
            None,
            &Default::default(),
        )?;
        placeholders.style(
            &mut frame,
            &models,
            |asset| source.digest(asset).is_none(),
            true,
        );
        for draw in &frame.draws {
            if matches!(draw.geometry, Geometry::Shape { .. }) {
                assert_eq!(draw.color, crate::PENDING_MODEL_COLOR);
            }
        }
        let model_count = frame
            .draws
            .iter()
            .filter(|draw| matches!(draw.geometry, Geometry::Model { .. }))
            .count();
        assert_eq!(model_count > 0, stage > 0);
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
        let look = crate::environment(&frame);
        let camera = CameraSpec::LookAt {
            position: glam30::Vec3::new(2.5, 5., 5.),
            target: glam30::Vec3::new(2.5, 0., 0.5),
            vertical_fov: 1.,
        };
        let mut baseline = None;
        for iteration in 0..24 {
            raster.prepare(renderer, &frame, plan, look, || {
                let mut content =
                    Content::resolve_available(&frame, &workspace, &schema, &library, &source)?;
                for draw in &frame.draws {
                    if matches!(draw.geometry, Geometry::Shape { .. }) {
                        content
                            .unpublished_models
                            .insert(draw.address.to_owned(), models[&draw.address].to_owned());
                    }
                }
                Ok(content)
            })?;
            if iteration == 0 {
                let arrivals = placeholders.take_arrivals();
                assert_eq!(arrivals.len(), usize::from(stage > 0));
                // Root arrivals now fade automatically, just like foliage,
                // from the previous generation's unpublished asset identities.
            }
            raster.render(renderer, &frame, &camera, ViewMode::Lit)?;
            if stage == 1 && matches!(iteration, 0 | 5) {
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
                    .write(&root.join(format!("fade-{iteration}.png")))?;
            }
            if stage > 0 && iteration == 0 {
                assert!(raster.needs_frame(), "arrival fade continues live");
            }
            if iteration == 12 {
                // Emitter discovery is independent worker work, not a fade. A
                // fixed number of tiny frames cannot guarantee it was scheduled
                // on a loaded machine; drain it before asserting complete idle.
                raster.wait_probe_gi(renderer)?;
            }
            if iteration >= 12 {
                assert!(!raster.needs_frame(), "completed fade has no idle work");
                assert_eq!(
                    renderer
                        .native_frame_work()
                        .context("native work")?
                        .material_rows,
                    0
                );
            }
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
        if stage == 3 {
            assert!(
                !placeholders.style(&mut frame, &models, |_| true, true),
                "a reimport with last-good model data never replaces it with loading bounds"
            );
            assert!(placeholders.take_arrivals().is_empty());
            let unique: BTreeSet<_> = frame
                .draws
                .iter()
                .filter_map(|draw| match &draw.geometry {
                    Geometry::Model { model, primitive } => Some((model.digest.0, *primitive)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                renderer
                    .native_geometry_stats()
                    .context("native geometry")?
                    .geometries,
                unique.len(),
                "different asset IDs still share identical cooked geometry"
            );
        }
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
        if stage == 0 {
            // The same bounds are conspicuous only for failed/unregistered IDs.
            let mut failed = document_frame(
                &entities,
                &transforms,
                &workspace,
                &mut library,
                &source,
                None,
                &Default::default(),
            )?;
            crate::ModelPlaceholders::default().style(&mut failed, &models, |_| false, true);
            for _ in 0..4 {
                raster.prepare(renderer, &failed, plan, look, || Ok(Content::default()))?;
                raster.render(renderer, &failed, &camera, ViewMode::Lit)?;
            }
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
                .write(&root.join("failed-magenta.png"))?;
            renderer.gpu().assert_clean()?;
        }
        if stage == 3 {
            // Captures finish cosmetic arrivals before measuring the scene.
            raster.fade_models(&BTreeSet::from(["as_panel0".into()]))?;
            raster.render(renderer, &frame, &camera, ViewMode::Lit)?;
            assert!(raster.needs_frame());
            raster.reset_capture(renderer)?;
            assert!(
                !raster.needs_frame(),
                "capture immediately finishes presentation fades"
            );
            raster.render(renderer, &frame, &camera, ViewMode::Lit)?;
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
