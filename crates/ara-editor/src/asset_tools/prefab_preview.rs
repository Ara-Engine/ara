//! Prefab thumbnails. A prefab's entities extract on the window thread exactly as the
//! Scene view draws them (prefab inheritance, procedural shapes, published models,
//! colours), then the thumbnail worker photographs them in the studio like a model.
//! Extraction needs the project store and model cache, which stay on that thread.
use super::thumbnails::Thumbnails;
use crate::Editor;
use std::{collections::BTreeMap, sync::Arc};

/// Extract and queue every waiting prefab request. A prefab whose content digest is
/// unchanged and whose image is still shown (`shown(key)`) is not rendered again.
pub(crate) fn extract(
    thumbnails: &mut Thumbnails,
    editor: &mut Editor,
    shown: impl Fn(&str) -> bool,
) {
    for (path, key, pixels) in thumbnails.take_prefabs() {
        let Some(digest) = digest_of(editor, &path) else {
            thumbnails.skip_prefab(&key);
            continue;
        };
        if shown(&key) && thumbnails.prefab_digests.get(&key) == Some(&digest) {
            continue;
        }
        match scene(editor, &path) {
            // Nothing visible (an empty or logic-only prefab) keeps its type icon.
            Some(scene) if !scene.meshes.is_empty() => {
                thumbnails
                    .prefab_digests
                    .insert(key.clone(), digest.clone());
                thumbnails.request_scene(&path, key, scene, digest, pixels);
            }
            _ => thumbnails.skip_prefab(&key),
        }
    }
}

/// The prefab at `path` as the Scene view would draw it, in prefab space.
fn scene(editor: &mut Editor, path: &str) -> Option<ara_render::scene::Scene> {
    let store = editor.host.store();
    let resolved = crate::world::resolve(store, path).ok()?;
    let view = crate::world::DocumentView::with_entities(store, Arc::new(resolved));
    let ids = crate::world::EditWorldView::entities(&view)
        .keys()
        .cloned()
        .collect();
    let state = crate::state::EditorState::default();
    view.extract(
        editor.host.root(),
        &state,
        None,
        &mut editor.models,
        Some(&ids),
    )
    .ok()
}

/// The resolved records plus the size and time of every model file they draw.
fn digest_of(editor: &Editor, path: &str) -> Option<String> {
    let store = editor.host.store();
    let workspace = store.workspace();
    let resolved = crate::world::resolve(store, path).ok()?;
    let mut bytes = Vec::new();
    let mut models = BTreeMap::new();
    for (id, entity) in &resolved {
        bytes.extend(id.as_bytes());
        bytes.extend(serde_json::to_vec(&entity.record).unwrap_or_default());
        let model = entity
            .record
            .get("MeshRenderer")
            .and_then(|r| r.get("model"))
            .and_then(serde_json::Value::as_str)
            .and_then(|r| ara_schema::grammar::parse_ref(r).ok())
            .and_then(|r| {
                r.id.and_then(|id| workspace.index().path_of(id))
                    .or(r.path)
                    .map(str::to_owned)
            });
        if let Some(model) = model {
            let stamp = std::fs::metadata(editor.host.root().join(workspace.storage_path(&model)))
                .ok()
                .map(|m| (m.len(), m.modified().ok()));
            models.insert(model, format!("{stamp:?}"));
        }
    }
    bytes.extend(format!("{models:?}").as_bytes());
    Some(ara_asset::Digest::of(&bytes).to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    /// A scratch project with `prefabs/pair.prefab` (a box with a red sphere child).
    fn project(name: &str) -> (std::path::PathBuf, Editor) {
        let root = crate::test_target()
            .join("prefab-preview")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let authored = root.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(authored.join("levels")).unwrap();
        std::fs::create_dir_all(authored.join("prefabs")).unwrap();
        let write = |path: std::path::PathBuf, value: serde_json::Value| {
            std::fs::write(path, ara_json::to_canonical_string(&value)).unwrap()
        };
        write(
            root.join("ara.project.json"),
            json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Prefab preview","main":"levels/main.scene"}),
        );
        write(
            authored.join("levels/main.scene"),
            json!({"$schema":"ara://schema/scene/1","id":"sc_prefabpv1","records":{}}),
        );
        write(
            authored.join("prefabs/pair.prefab"),
            json!({"$schema":"ara://schema/prefab/1","id":"pf_pairpv01","records":{
                "e_pairbase":{"name":"Base","order":"a0","Transform":{},"MeshRenderer":{"shape":"box"}},
                "e_pairtop1":{"name":"Top","parent":"e_pairbase","order":"a1","Transform":{"position":[0,1,0]},"MeshRenderer":{"shape":"sphere","color":[1,0,0,1]}}
            }}),
        );
        let editor = Editor::open_with(&root, Some(root.join(".ara/test-user.json"))).unwrap();
        (root, editor)
    }

    #[test]
    fn prefabs_extract_their_entities_with_a_stable_content_digest() {
        let (root, mut editor) = project("extract");
        let extracted = scene(&mut editor, "prefabs/pair.prefab").expect("prefab extracts");
        assert_eq!(extracted.meshes.len(), 2, "both entities draw");
        let top = extracted
            .meshes
            .iter()
            .find(|m| m.name == "Top")
            .expect("the child draws under its own name");
        // The child sits a metre above the base, as the Scene view places it.
        let lowest = top
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::INFINITY, f32::min);
        assert!(
            lowest > 0.4,
            "child placed by its parent's transform: {lowest}"
        );
        let digest = digest_of(&editor, "prefabs/pair.prefab").unwrap();
        assert_eq!(digest_of(&editor, "prefabs/pair.prefab").unwrap(), digest);
        assert!(scene(&mut editor, "prefabs/missing.prefab").is_none());
        drop(editor);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[ignore = "needs a Vulkan GPU; writes target/prefab-preview/pair.png"]
    fn prefab_thumbnails_render_the_prefabs_meshes_in_the_studio() {
        let (root, mut editor) = project("shot");
        let extracted = scene(&mut editor, "prefabs/pair.prefab").unwrap();
        let image = super::super::thumbnails::prefab_shot(&root, extracted, "test").unwrap();
        let red = image
            .pixels()
            .filter(|p| p[0] > 120 && p[1] < 80 && p[2] < 80)
            .count();
        assert!(red > 500, "the red sphere child is in the shot ({red} px)");
        image
            .save(crate::test_target().join("prefab-preview").join("pair.png"))
            .unwrap();
        drop(editor);
        let _ = std::fs::remove_dir_all(&root);
    }
}
