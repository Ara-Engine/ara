//! Named camera views are navigation edits; recall obeys the caller's presence scope.
use super::*;

pub(super) fn run(
    state: &mut EditorState,
    cx: &Cx<'_>,
    params: EditorCameraBookmark,
    human: bool,
) -> R {
    let scene = match params.scene {
        None => state.doc.clone(),
        Some(scene) => cx
            .store()
            .workspace()
            .find(&scene)
            .ok_or_else(|| vec![crate::invalid("Bookmark scene is not loaded")])?
            .to_string(),
    };
    if cx
        .store()
        .workspace()
        .doc(&scene)
        .is_none_or(|doc| !matches!(doc.kind.as_str(), "scene" | "prefab"))
    {
        return Err(vec![crate::invalid(
            "Bookmarks belong to a loaded scene or prefab",
        )]);
    }
    if matches!(params.action, CameraBookmarkAction::List) {
        return Ok(
            json!({"scene":scene,"bookmarks":state.camera_bookmarks.get(&scene).into_iter().flat_map(|bookmarks|bookmarks.iter())
            .map(|(name,camera)|json!({"name":name,"camera":camera})).collect::<Vec<_>>() }),
        );
    }
    let name = match params.name {
        Some(name) => {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
                return Err(vec![crate::invalid(
                    "Bookmark names use 1–120 visible characters",
                )]);
            }
            name.to_owned()
        }
        None if matches!(params.action, CameraBookmarkAction::Save) => (1_u64..)
            .map(|index| format!("View {index}"))
            .find(|name| {
                state
                    .camera_bookmarks
                    .get(&scene)
                    .is_none_or(|bookmarks| !bookmarks.contains_key(name))
            })
            .ok_or_else(|| vec![crate::invalid("No unused bookmark name remains")])?,
        None => return Err(vec![crate::invalid("Choose a named camera view")]),
    };
    match params.action {
        CameraBookmarkAction::Save => {
            if scene != state.doc {
                return Err(vec![crate::invalid(
                    "Open this scene before saving its current view",
                )]);
            }
            if !state.camera.is_valid() {
                return Err(vec![crate::invalid("Cannot save an invalid camera view")]);
            }
            state
                .camera_bookmarks
                .entry(scene.clone())
                .or_default()
                .insert(name.clone(), state.camera);
        }
        CameraBookmarkAction::Recall => {
            let camera = state
                .camera_bookmarks
                .get(&scene)
                .and_then(|bookmarks| bookmarks.get(&name))
                .copied()
                .ok_or_else(|| vec![crate::invalid("This camera bookmark no longer exists")])?;
            if !camera.is_valid() {
                return Err(vec![crate::invalid("This camera bookmark is invalid")]);
            }
            if state.doc != scene {
                if human {
                    state.scene_cameras.insert(state.doc.clone(), state.camera);
                }
                state.doc = scene.clone();
                state.selection.clear();
                if human {
                    state.recent_scenes.retain(|id| id != &scene);
                    state.recent_scenes.insert(0, scene.clone());
                    state.recent_scenes.truncate(10);
                }
            }
            state.camera = camera;
        }
        CameraBookmarkAction::Delete => {
            let bookmarks = state
                .camera_bookmarks
                .get_mut(&scene)
                .ok_or_else(|| vec![crate::invalid("This scene has no camera bookmarks")])?;
            if bookmarks.remove(&name).is_none() {
                return Err(vec![crate::invalid(
                    "This camera bookmark no longer exists",
                )]);
            }
            if bookmarks.is_empty() {
                state.camera_bookmarks.remove(&scene);
            }
        }
        CameraBookmarkAction::List => {}
    }
    Ok(json!({"scene":scene,"name":name}))
}
