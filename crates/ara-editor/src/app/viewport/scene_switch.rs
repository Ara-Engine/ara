//! Offscreen reproduction of the real SceneSync scene-open boundary.
use super::*;
use crate::world::EditWorldView;

#[test]
#[ignore = "Outpost assets and GPU required; set ARA_SCENE_SWITCH_PROJECT and ARA_SCENE_SWITCH_OUTPUT"]
fn outpost_hangar_round_trip() -> anyhow::Result<()> {
    let project = PathBuf::from(std::env::var("ARA_SCENE_SWITCH_PROJECT")?);
    let output = PathBuf::from(std::env::var("ARA_SCENE_SWITCH_OUTPUT")?);
    std::fs::create_dir_all(&output)?;
    let mut editor = Editor::open(&project)?;
    let _assets =
        ara_play_host::PublishedAssets::for_project(&project).map_err(anyhow::Error::msg)?;
    let worker = ara_play_host::PlayConfig::for_project(&project).import_worker;
    ara_play_host::package_scene_with_progress(&project, &worker, |progress| {
        eprintln!(
            "Scene switch imports: {}/{}",
            progress.completed, progress.total
        );
        Ok(())
    })
    .map_err(anyhow::Error::msg)?;
    editor.sync();
    editor.action("editor.scene.open", json!({"scene":"levels/outpost.scene"}))?;
    editor.action("editor.preferences", json!({"renderQuality":"high"}))?;
    let view = editor.view()?;
    let mut renderer = Renderer::new_raster(
        view.scene(&project, &editor.state, None, &mut editor.models)?,
        [1280, 720],
        &[],
        project.join(".ara/gpu"),
    )?;
    let mut scene = SceneView::default();
    let mut sync = super::super::scene_sync::SceneSync::new(
        editor.models.revision(),
        editor.host.store().revision(),
    );
    let result = (|| -> anyhow::Result<()> {
        let mut rows = Vec::new();
        let mut cameras = std::collections::BTreeMap::new();
        for (name, path) in [
            ("01-outpost", "levels/outpost.scene"),
            ("02-hangar", "levels/hangar.scene"),
            ("03-outpost-return", "levels/outpost.scene"),
        ] {
            editor.action("editor.scene.open", json!({"scene":path}))?;
            let view = editor.view()?;
            sync.rebuild(&view, &mut editor, &mut renderer, &mut scene);
            if let Some(saved) = cameras.get(path) {
                assert_eq!(
                    &editor.state.camera, saved,
                    "reopening restores scene camera"
                );
            }
            cameras.insert(path, editor.state.camera);
            if name == "02-hangar" {
                let request = CaptureRequest {
                    camera: editor.state.camera.spec(),
                    size: [1280, 720],
                    mode: ViewMode::Lit,
                    deterministic: true,
                    exposure: 0.0015,
                    settle_frames: 1,
                    tick: 0,
                    seed: 0,
                    marks: None,
                };
                let before = renderer.capture_completed(&request)?.image;
                scene
                    .render(&mut renderer, &editor, &view, 1)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                renderer.wait_scene_frame()?;
                assert!(scene.preview_view.is_none(), "outgoing draft is discarded");
                if scene.raster_scene.as_deref() != Some(editor.state.doc.as_str()) {
                    assert_eq!(
                        renderer.capture_completed(&request)?.image,
                        before,
                        "an in-flight scene switch retains the completed image and its camera"
                    );
                }
            }
            scene
                .render_capture(&mut renderer, &editor, &view, 64)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let frame = scene.raster_frame.as_ref().expect("published scene");
            let mut expected = crate::world::gltf::FidelityContent::new(&project)?;
            let expected = expected.frame(
                &view,
                editor.host.store().workspace(),
                editor.host.store().schema(),
                None,
                &editor.state.hidden,
            )?;
            let request = CaptureRequest {
                camera: editor.state.camera.spec(),
                size: [1280, 720],
                mode: ViewMode::Lit,
                deterministic: true,
                exposure: frame.environment.exposure,
                settle_frames: 64,
                tick: 0,
                seed: 0,
                marks: None,
            };
            scene
                .capture(&mut renderer, &request)?
                .write(&output.join(format!("{name}.png")))?;
            rows.push(json!({"scene":path,"camera":editor.state.camera,
                "draws":frame.draws.len(),"lights":frame.lights.len(),
                "exposure":frame.environment.exposure,
                "sky":format!("{:?}",frame.look.sky),
                "expectedSky":format!("{:?}",expected.look.sky)}));
            std::fs::write(
                output.join("scene-switch.json"),
                ara_json::to_canonical_string(&json!(rows)),
            )?;
            assert_eq!(
                frame.look, expected.look,
                "scene look must replace previous look"
            );
            assert_eq!(frame.environment, expected.environment);
            assert_eq!(frame.sun, expected.sun);
            assert_eq!(frame.lights, expected.lights);
            assert_eq!(frame.effects, expected.effects);
            let materials = |frame: &ara_runtime::extract::RenderFrame| {
                frame
                    .draws
                    .iter()
                    .map(|draw| (draw.address.clone(), draw.color, draw.emissive))
                    .collect::<Vec<_>>()
            };
            assert_eq!(materials(frame), materials(&expected));
            if path.ends_with("hangar.scene") {
                // Same camera as the shipped player's reviewed bay capture.
                let mut camera = crate::state::Camera::default();
                camera.look_from(Vec3::new(3.2, 1.65, -11.4), Vec3::new(-4.7, -0.05, 17.4));
                editor.action("editor.camera", json!({"target":camera.target.to_array(),
                    "yaw":camera.yaw,"pitch":camera.pitch,"distance":camera.distance,"fovYDegrees":60}))?;
                scene
                    .render_capture(&mut renderer, &editor, &view, 64)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                let mut request = request;
                request.camera = editor.state.camera.spec();
                scene
                    .capture(&mut renderer, &request)?
                    .write(&output.join("02-hangar-bay.png"))?;
            }
            if name == "01-outpost" {
                // Leave a transient Inspector projection behind: switching must
                // not use that outgoing document to prepare the Hangar frame.
                scene.set_preview_view(
                    &editor.state.doc,
                    Some(view.with_records(Default::default())),
                );
            }
        }
        renderer.gpu().assert_clean()?;
        Ok(())
    })();
    scene
        .destroy(&mut renderer)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    result
}
