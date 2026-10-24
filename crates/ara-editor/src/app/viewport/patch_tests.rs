#![allow(clippy::unwrap_used)] // Fixture invariants fail the focused GPU test directly.
use super::*;
use crate::world::{DocumentView, EditWorldView, Patch};
use std::collections::{BTreeMap, BTreeSet};

fn preview(
    view: &DocumentView,
    id: &str,
    change: impl FnOnce(&mut ara_doc::Record),
) -> DocumentView {
    let mut record = view.entities()[id].record.clone();
    change(&mut record);
    view.with_records(BTreeMap::from([(id.to_owned(), record)]))
}

fn pose_patch() -> Patch {
    Patch {
        moved: BTreeSet::from(["e_box".into()]),
        ..Default::default()
    }
}

fn root(scene: &SceneView) -> &ara_runtime::extract::Draw {
    scene
        .raster_frame
        .as_ref()
        .unwrap()
        .draws
        .iter()
        .find(|draw| draw.address == "e_box")
        .unwrap()
}

pub(super) fn exercise(
    scene: &mut SceneView,
    renderer: &mut Renderer,
    editor: &mut Editor,
    view: &DocumentView,
    output: &std::path::Path,
) -> anyhow::Result<()> {
    scene.raster.as_mut().unwrap().set_probe_gi(false);
    let frame = scene.raster_frame.as_ref().unwrap().draws.as_ptr();
    let descriptors = renderer.gpu().descriptor_usage();
    let apply = |scene: &mut SceneView,
                 renderer: &mut Renderer,
                 editor: &Editor,
                 target,
                 patch: &Patch|
     -> anyhow::Result<()> {
        assert!(
            scene
                .apply_patch(renderer, editor, view, target, patch)
                .map_err(|error| anyhow::anyhow!("{error}"))?
        );
        scene
            .render(renderer, editor, view, 1)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        assert_eq!(
            scene.raster_frame.as_ref().unwrap().draws.as_ptr(),
            frame,
            "a value edit must retain the extracted frame allocation"
        );
        assert_eq!(
            renderer.gpu().descriptor_usage(),
            descriptors,
            "value edits must retain native GPU allocations"
        );
        renderer.gpu().assert_clean()?;
        Ok(())
    };
    let moved = preview(view, "e_box", |row| {
        row["Transform"] = json!({"position":[2,0,0]});
    });
    apply(scene, renderer, editor, Some(moved), &pose_patch())?;
    assert_eq!(root(scene).world.w_axis.x, 2.);
    apply(scene, renderer, editor, None, &pose_patch())?;
    assert_eq!(root(scene).world.w_axis.x, 0.);

    let factors = Patch {
        materials: true,
        ..Default::default()
    };
    let coloured = preview(view, "e_box", |row| {
        row.get_mut("MeshRenderer").unwrap()["color"] = json!([0.8, 0.1, 0.2, 1]);
        row.get_mut("MeshRenderer").unwrap()["roughness"] = json!(0.15);
    });
    apply(scene, renderer, editor, Some(coloured), &factors)?;
    assert_eq!(root(scene).color, [0.8, 0.1, 0.2, 1.]);
    assert_eq!(root(scene).roughness, 0.15);
    renderer
        .capture_completed(&CaptureRequest {
            camera: editor.state.camera.spec(),
            size: renderer.size(),
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: 0.0015,
            settle_frames: 1,
            tick: 0,
            seed: 7,
            marks: None,
        })?
        .write(&output.join("patch-colour.png"))?;
    apply(scene, renderer, editor, None, &factors)?;
    assert_eq!(root(scene).color, [0.15, 0.5, 0.2, 1.]);

    let names = Patch {
        renamed: BTreeSet::from(["e_box".into()]),
        ..Default::default()
    };
    let renamed = preview(view, "e_box", |row| {
        row.insert("name".into(), json!("Renamed box"));
    });
    apply(scene, renderer, editor, Some(renamed), &names)?;
    let entity = root(scene).entity;
    assert!(
        renderer
            .entities()
            .iter()
            .any(|(id, name)| *id == entity && name == "Renamed box (e_box)")
    );
    apply(scene, renderer, editor, None, &names)?;
    assert!(
        renderer
            .entities()
            .iter()
            .any(|(id, name)| *id == entity && name == "Box (e_box)")
    );

    let lights = Patch {
        lights: true,
        ..Default::default()
    };
    let dimmed = preview(view, "e_sun", |row| {
        row.get_mut("Light").unwrap()["intensity"] = json!(12000);
    });
    apply(scene, renderer, editor, Some(dimmed), &lights)?;
    assert_eq!(
        scene.raster_frame.as_ref().unwrap().sun.unwrap().1,
        [12000.; 3]
    );
    apply(scene, renderer, editor, None, &lights)?;
    assert_eq!(
        scene.raster_frame.as_ref().unwrap().sun.unwrap().1,
        [24000.; 3]
    );

    editor.state.selection = vec!["e_box".into()];
    editor.state.snapping = false;
    editor.gesture = Some(crate::world::gesture::Gesture::begin(
        view.entities(),
        &editor.state,
        editor.host.store().revision(),
    )?);
    for step in 1..=8 {
        editor
            .gesture
            .as_mut()
            .unwrap()
            .update(Vec3::new(step as f32 * 0.25, 0., 0.));
        apply(scene, renderer, editor, None, &pose_patch())?;
        assert_eq!(root(scene).world.w_axis.x, step as f32 * 0.25);
    }
    editor.gesture = None;
    editor.state.selection.clear();
    apply(scene, renderer, editor, None, &pose_patch())?;
    assert_eq!(root(scene).world.w_axis.x, 0.);
    scene.raster.as_mut().unwrap().set_probe_gi(true);
    Ok(())
}
