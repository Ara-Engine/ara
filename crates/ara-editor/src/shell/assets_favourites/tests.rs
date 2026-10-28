//! Real navigation operations, retained input and layout persistence for bookmarks.
use super::*;
mod fixture;
use fixture::{Clip, Fixture, Project, navigation_revision};

#[test]
fn favourites_and_scoped_saved_searches_are_single_navigation_transactions() -> ara_ui::Result<()> {
    let mut f = Fixture::new([1100., 700.])?;
    let host_revision = f.editor.host.store().revision();
    let camera = f.editor.state.camera;
    let entities = f.editor.state.selection.clone();
    let initial = navigation_revision(&mut f.editor)?;
    f.shell
        .toggle_asset_favourite(&mut f.editor, &Target::Folder("game/Art".into()))?;
    assert_eq!(navigation_revision(&mut f.editor)?, initial + 1);
    assert_eq!(
        f.editor.state.asset_favourites["game/Art"],
        AssetFavouriteKind::Folder
    );
    f.shell
        .toggle_asset_favourite(&mut f.editor, &Target::Folder("game/Art".into()))?;
    assert_eq!(navigation_revision(&mut f.editor)?, initial + 2);
    assert!(!f.editor.state.asset_favourites.contains_key("game/Art"));
    f.shell
        .toggle_asset_favourite(&mut f.editor, &Target::Asset("game/Art/a.prefab".into()))?;
    assert!(
        f.editor
            .state
            .asset_favourites
            .contains_key("game/Art/a.prefab")
    );
    assert!(
        !f.editor.state.asset_favourites.contains_key("Art/a.prefab"),
        "Storage identities must not silently become authored-relative paths"
    );

    f.editor.action(
        "editor.workspace",
        json!({"folder":"game/Art","assetQuery":"type:prefab","folderOnly":true}),
    )?;
    f.shell.rebuild(&mut f.editor)?;
    let paths: std::collections::BTreeSet<_> = f
        .shell
        .asset_entries
        .values()
        .map(|asset| asset.path.as_str())
        .collect();
    assert!(paths.contains("game/Art/a.prefab"));
    assert!(paths.contains("game/Art/Nested/b.prefab"));
    assert!(!paths.contains("game/Artist/outside.prefab"));
    let before_save = navigation_revision(&mut f.editor)?;
    f.shell
        .save_asset_search(&mut f.editor, &Target::Text("Art prefabs".into()))?;
    assert_eq!(navigation_revision(&mut f.editor)?, before_save + 1);
    assert_eq!(
        f.editor.state.asset_saved_searches["Art prefabs"],
        SavedAssetSearch {
            query: "type:prefab".into(),
            folder: "game/Art".into(),
            folder_only: true
        }
    );
    f.editor.action(
        "editor.workspace",
        json!({"folder":"game/Artist","assetQuery":"outside","folderOnly":false}),
    )?;
    let before_apply = navigation_revision(&mut f.editor)?;
    f.shell
        .use_asset_search(&mut f.editor, &Target::Text("Art prefabs".into()), false)?;
    assert_eq!(navigation_revision(&mut f.editor)?, before_apply + 1);
    assert_eq!(f.editor.state.asset_folder, "game/Art");
    assert_eq!(f.editor.state.searches["assets"], "type:prefab");
    assert!(f.editor.state.asset_search_folder_only);
    assert_eq!(f.editor.host.store().revision(), host_revision);
    assert_eq!(f.editor.state.camera, camera);
    assert_eq!(f.editor.state.selection, entities);
    Ok(())
}

#[test]
fn scoped_restore_preserves_later_entries_and_current_navigation_and_refuses_name_conflicts()
-> ara_ui::Result<()> {
    let mut f = Fixture::new([1100., 700.])?;
    f.editor.action("editor.workspace", json!({"assetQuery":"type:prefab","folder":"game/Art","folderOnly":true,"saveSearch":"Art"}))?;
    let original = f.editor.state.asset_saved_searches["Art"].clone();
    f.shell
        .use_asset_search(&mut f.editor, &Target::Text("Art".into()), true)?;
    f.editor.action("editor.workspace", json!({"assetQuery":"outside","folder":"game/Artist","folderOnly":false,"saveSearch":"Later"}))?;
    let camera = f.editor.state.camera;
    let selection = f.editor.state.selection.clone();
    let later = f.editor.state.asset_saved_searches["Later"].clone();
    let host_revision = f.editor.host.store().revision();
    let before_restore = navigation_revision(&mut f.editor)?;
    f.shell.restore_asset_search(&mut f.editor)?;
    assert_eq!(navigation_revision(&mut f.editor)?, before_restore + 1);
    assert_eq!(f.editor.state.asset_saved_searches["Art"], original);
    assert_eq!(f.editor.state.asset_saved_searches["Later"], later);
    assert_eq!(f.editor.state.asset_folder, "game/Artist");
    assert_eq!(f.editor.state.searches["assets"], "outside");
    assert!(!f.editor.state.asset_search_folder_only);
    assert_eq!(f.editor.state.camera, camera);
    assert_eq!(f.editor.state.selection, selection);
    assert_eq!(f.editor.host.store().revision(), host_revision);

    f.shell
        .use_asset_search(&mut f.editor, &Target::Text("Art".into()), true)?;
    f.editor.action(
        "editor.workspace",
        json!({"saveSearch":"Art","savedSearch":later}),
    )?;
    let before_conflict = f.editor.state.clone();
    let navigation = navigation_revision(&mut f.editor)?;
    assert!(f.shell.restore_asset_search(&mut f.editor).is_err());
    assert_eq!(f.editor.state, before_conflict);
    assert_eq!(navigation_revision(&mut f.editor)?, navigation);
    Ok(())
}

#[test]
fn native_naming_prompt_saves_captured_scope_without_replacing_later_navigation()
-> ara_ui::Result<()> {
    let mut f = Fixture::new([1100., 700.])?;
    f.editor.action(
        "editor.workspace",
        json!({"folder":"game/Art","assetQuery":"type:prefab","folderOnly":true}),
    )?;
    f.shell.rebuild(&mut f.editor)?;
    let original = f.shell.current_asset_search(&f.editor);
    f.shell.save_asset_search(&mut f.editor, &Target::None)?;
    assert!(f.shell.confirmations.is_open());
    assert!(f.shell.native_text("Captured Art", &mut Clip)?);
    f.shell.dispatch(&mut f.editor)?;
    // A background navigation notification can arrive while naming. The accepted
    // prompt saves its captured query and does not apply it to that later view.
    f.editor.action(
        "editor.workspace",
        json!({"folder":"game/Artist","assetQuery":"outside","folderOnly":false}),
    )?;
    f.shell.rebuild(&mut f.editor)?;
    f.shell.key(&mut f.editor, 40, true, &mut Clip)?;
    f.shell.key(&mut f.editor, 40, false, &mut Clip)?;
    f.shell.poll_asset_favourites(&mut f.editor)?;
    assert_eq!(
        f.editor.state.asset_saved_searches.get("Captured Art"),
        Some(&original)
    );
    assert_eq!(f.editor.state.asset_folder, "game/Artist");
    assert_eq!(f.editor.state.searches["assets"], "outside");
    assert!(!f.editor.state.asset_search_folder_only);
    Ok(())
}

#[test]
fn bookmark_toolbar_and_shared_picker_use_real_input_at_narrow_widths() -> ara_ui::Result<()> {
    let mut f = Fixture::new([640., 480.])?;
    f.editor
        .action("editor.workspace", json!({"folder":"game/Art","asset":""}))?;
    f.shell.rebuild(&mut f.editor)?;
    f.shell.click("assets-favourite-toggle", &mut Clip)?;
    f.shell.dispatch(&mut f.editor)?;
    assert!(f.editor.state.asset_favourites.contains_key("game/Art"));
    for name in ["A textures", "B audio", "C models", "Z saved prefabs"] {
        f.editor.action("editor.workspace", json!({"saveSearch":name,"savedSearch":{"query":"type:prefab","folder":"game/Art","folderOnly":true}}))?;
    }
    f.shell.ui.resize([320., 480.])?;
    f.shell.rebuild(&mut f.editor)?;
    f.shell.frame()?;
    for id in [
        "asset-type-select",
        "asset-bookmark-select",
        "asset-search-folder-only",
    ] {
        let index = f
            .shell
            .shown_node(id)
            .ok_or("Bookmark controls must remain mounted in narrow Assets")?;
        let node = &f.shell.ui.nodes[index];
        let visible = node.rect.intersection(node.clip);
        assert!(
            node.rect.w > 1.
                && node.rect.h > 1.
                && visible.w + 0.5 >= node.rect.w
                && visible.h + 0.5 >= node.rect.h,
            "{id}: rect {:?}, clip {:?}",
            node.rect,
            node.clip
        );
    }
    f.shell.click("asset-bookmark-select", &mut Clip)?;
    f.shell.dispatch(&mut f.editor)?;
    assert!(f.shell.choice_open());
    for key in [77, 40] {
        f.shell.key(&mut f.editor, key, true, &mut Clip)?;
        f.shell.key(&mut f.editor, key, false, &mut Clip)?;
    }
    assert!(!f.shell.choice_open());
    assert_eq!(f.editor.state.searches["assets"], "type:prefab");
    assert_eq!(f.editor.state.asset_folder, "game/Art");
    assert!(f.editor.state.asset_search_folder_only);
    f.shell.click("asset-search-folder-only", &mut Clip)?;
    f.shell.dispatch(&mut f.editor)?;
    assert!(!f.editor.state.asset_search_folder_only);
    assert!(
        f.shell
            .asset_entries
            .values()
            .any(|asset| asset.path == "game/Artist/outside.prefab")
    );
    let saved_count = f.editor.state.asset_saved_searches.len();
    f.shell
        .use_asset_search(&mut f.editor, &Target::None, true)?;
    assert!(f.shell.choice_open());
    f.shell.key(&mut f.editor, 41, true, &mut Clip)?;
    f.shell.key(&mut f.editor, 41, false, &mut Clip)?;
    f.shell.poll_asset_favourites(&mut f.editor)?;
    f.shell.click("asset-bookmark-select", &mut Clip)?;
    f.shell.dispatch(&mut f.editor)?;
    for key in [77, 40] {
        f.shell.key(&mut f.editor, key, true, &mut Clip)?;
        f.shell.key(&mut f.editor, key, false, &mut Clip)?;
    }
    assert_eq!(
        f.editor.state.asset_saved_searches.len(),
        saved_count,
        "Cancelling Remove must not turn the ordinary picker into a destructive action"
    );
    assert!(
        f.editor.state.asset_search_folder_only,
        "Normal selection applies the saved search after cancelled removal"
    );
    Ok(())
}

#[test]
fn layout_bookmarks_round_trip_and_old_version_two_layouts_remain_compatible() -> ara_ui::Result<()>
{
    let project = Project::new()?;
    let mut state = crate::state::EditorState::default();
    state
        .asset_favourites
        .insert("game/Art".into(), AssetFavouriteKind::Folder);
    state.asset_saved_searches.insert(
        "Art prefabs".into(),
        SavedAssetSearch {
            query: "type:prefab".into(),
            folder: "game/Art".into(),
            folder_only: true,
        },
    );
    state.asset_search_folder_only = true;
    let path = project.0.join(".ara/editor-layout.json");
    std::fs::create_dir_all(path.parent().ok_or("Layout parent missing")?)?;
    crate::state::write_layout(&path, &state)?;
    let restored = crate::state::Navigation::new(&project.0, Default::default())?;
    assert_eq!(restored.state.asset_favourites, state.asset_favourites);
    assert_eq!(
        restored.state.asset_saved_searches,
        state.asset_saved_searches
    );
    assert!(restored.state.asset_search_folder_only);
    drop(restored);
    let dock = ara_json::parse(&state.dock()?.serialize()?)?;
    std::fs::write(
        &path,
        ara_json::to_canonical_string(
            &json!({"version":2,"layout":dock,"assets":{"sidebarWidth":180}}),
        ),
    )?;
    let old = crate::state::Navigation::new(&project.0, Default::default())?;
    assert!(old.state.asset_favourites.is_empty());
    assert!(old.state.asset_saved_searches.is_empty());
    assert!(!old.state.asset_search_folder_only);
    assert_eq!(old.state.asset_sidebar_width, 180.);
    Ok(())
}
