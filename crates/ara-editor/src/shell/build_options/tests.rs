//! Build-form geometry under the real editor resets; exporting never runs here.
#![allow(clippy::unwrap_used)]
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn only_declared_canonical_and_indexed_legacy_profile_paths_are_accepted() {
    assert_eq!(
        profile_name("settings/builds/Windows shipping.export.json"),
        Some(("Windows shipping", true))
    );
    assert_eq!(
        profile_name("presets/mobile.export.json"),
        Some(("mobile", false))
    );
    for path in [
        "game/settings/builds/game.export.json",
        "settings/builds/.export.json",
        "settings/builds/nested/a.export.json",
        "settings/builds/export.json",
        "other/a.export.json",
    ] {
        assert!(profile_name(path).is_none(), "{path}");
    }
}
#[test]
fn a_removed_explicit_profile_never_builds_a_different_first_profile() {
    let names = vec!["mobile".into(), build::DEFAULT_PROFILE.into()];
    assert_eq!(
        selected_profile(&names, "").ok(),
        Some(build::DEFAULT_PROFILE)
    );
    assert_eq!(selected_profile(&names, "mobile").ok(), Some("mobile"));
    assert!(selected_profile(&names, "deleted").is_err());
    assert!(selected_profile(&[], "").is_err());
}

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
        let root = target.join("build-options-layout").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Build form layout","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_buildform","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn visible(shell: &Shell, id: &str) -> ara_ui::paint::Box2 {
    let index = shell
        .shown_node(id)
        .unwrap_or_else(|| panic!("{id} must be mounted and shown"));
    let node = &shell.ui.nodes[index];
    let clipped = node.rect.intersection(node.clip);
    assert!(
        clipped.w > 0. && clipped.h > 0.,
        "{id}: {:?} clipped by {:?}",
        node.rect,
        node.clip
    );
    clipped
}
fn bounded(shell: &Shell, id: &str) {
    let index = shell.shown_node(id).unwrap();
    let rect = shell.ui.nodes[index].rect;
    assert!(
        rect.x >= -0.5
            && rect.y >= -0.5
            && rect.x + rect.w <= shell.ui.size[0] + 0.5
            && rect.y + rect.h <= shell.ui.size[1] + 0.5,
        "{id}: {rect:?} in {:?}",
        shell.ui.size
    );
}

#[test]
fn build_form_keeps_intrinsic_profile_and_help_scrollable_at_narrow_two_hundred_percent_scale() {
    // The native host divides physical dimensions by scale before Ui layout.
    // These include a 320px physical window at 200%, not just a 320px logical one.
    // This proves retained layout at the host's logical extent, not GPU raster scale.
    let mut measurements = Vec::new();
    for (physical, scale) in [([640., 900.], 2.), ([320., 900.], 2.), ([640., 480.], 2.)] {
        let logical = physical.map(|side| side / scale);
        let project = Project::new();
        let mut editor = Editor::open(&project.0).unwrap();
        let mut shell = Shell::new(&mut editor, logical).unwrap();
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while shell.browser.revision == 0
            && shell.browser.files.is_empty()
            && shell.browser.error.is_none()
            && std::time::Instant::now() < limit
        {
            shell.browser.poll();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(shell.browser.revision > 0 || !shell.browser.files.is_empty());
        let names:Vec<_>=(0..12).map(|index|format!("Profile {index:02} — localized Windows shipping configuration · إعدادات البناء والتوزيع")).collect();
        shell.browser.files = names
            .iter()
            .map(|name| {
                let path = format!("settings/builds/{name}.export.json");
                AuthoredFile {
                    path: path.clone(),
                    storage_path: path,
                    scope: AuthoredScope::Project,
                    role: AuthoredRole::BuildPreset,
                }
            })
            .collect();
        shell.browser.revision += 1;
        assert_eq!(
            shell.build_profiles().len(),
            12,
            "the long-profile fixture must reach the real Select branch"
        );
        let revision = editor.host.store().revision();
        let selection = editor.state.selection.clone();
        let camera = editor.state.camera;
        shell.open_build_options(&mut editor).unwrap();
        shell.frame().unwrap();
        bounded(&shell, "build-options-body");
        bounded(&shell, "build-options-actions");
        let body = visible(&shell, "build-options-scroll");
        let control = visible(&shell, CONTROL);
        assert!(body.h > 1. && control.h > 1.);
        for id in [
            "build-options-close",
            "build-options-destination",
            "build-options-build",
        ] {
            visible(&shell, id);
            bounded(&shell, id);
        }
        let index = shell.ui.find(CONTROL).unwrap();
        let node = &shell.ui.nodes[index];
        let layout = shell.ui.tree.layout(node.layout_id).unwrap();
        let text_height = node.text.as_ref().unwrap().layout.height();
        let text_width = node.text.as_ref().unwrap().layout.width();
        let available_width = node.rect.w
            - layout.padding.left
            - layout.padding.right
            - layout.border.left
            - layout.border.right;
        let available_height = node.rect.h
            - layout.padding.top
            - layout.padding.bottom
            - layout.border.top
            - layout.border.bottom;
        assert!(
            text_width <= available_width + 0.5,
            "wrapped profile text must fit its content width: {text_width} > {available_width}; {:?} in {logical:?}",
            node.rect
        );
        assert!(
            text_height <= available_height + 0.5,
            "wrapped profile text must retain its intrinsic height: {text_height} > {available_height}; {:?} in {logical:?}",
            node.rect
        );
        let scroll = shell.ui.find("build-options-scroll").unwrap();
        let point = [body.x + body.w * 0.5, body.y + body.h * 0.5];
        let mut clipboard = crate::native::SessionClipboard::new(true);
        shell
            .event(
                InputEvent::Wheel {
                    position: point,
                    delta: [0., 10_000.],
                },
                &mut clipboard,
            )
            .unwrap();
        shell.frame().unwrap();
        visible(&shell, "build-options-output-note");
        let offset = shell.ui.scroll_position(scroll).unwrap()[1];
        let contents = shell.ui.nodes[scroll]
            .element
            .children
            .iter()
            .map(|index| shell.ui.nodes[*index].rect.h)
            .sum::<f32>();
        assert!(
            contents <= body.h + 0.5 || offset > 0.,
            "constrained form content must scroll rather than shrink to zero"
        );
        shell.ui.scroll_into_view(index).unwrap();
        shell.frame().unwrap();
        visible(&shell, CONTROL);
        shell.key(&mut editor, 40, true, &mut clipboard).unwrap();
        shell.frame().unwrap();
        assert!(shell.choice_open());
        visible(&shell, "choice-build-profile-select-search");
        bounded(&shell, "choice-build-profile-select-popover");
        shell.key(&mut editor, 77, true, &mut clipboard).unwrap();
        shell.frame().unwrap();
        let last = shell
            .ui
            .nodes
            .iter()
            .position(|node| {
                node.element
                    .id
                    .starts_with("choice-build-profile-select-option-")
                    && node.element.attrs.get("label") == names.last()
            })
            .unwrap();
        let last = &shell.ui.nodes[last];
        let clip = last.rect.intersection(last.clip);
        assert!(
            clip.w > 0. && clip.h >= last.rect.h - 0.5,
            "last long profile remains fully reachable: {:?} clipped by {:?}",
            last.rect,
            last.clip
        );
        measurements.push(json!({"physical":physical,"scale":scale,"logical":logical,"bodyClip":body,"controlClip":control,"helpScrollOffset":offset,"textHeight":text_height,"lastOptionBox":last.rect,"lastOptionClip":clip,"popupBox":shell.ui.nodes[shell.ui.find("choice-build-profile-select-popover").unwrap()].rect}));
        shell.key(&mut editor, 41, true, &mut clipboard).unwrap();
        assert!(shell.build_options_open());
        shell.key(&mut editor, 41, true, &mut clipboard).unwrap();
        assert!(!shell.build_options_open());
        assert_eq!(editor.host.store().revision(), revision);
        assert_eq!(editor.state.selection, selection);
        assert_eq!(editor.state.camera, camera);
        assert!(
            !project.0.join("builds").exists(),
            "layout/input proof never exports or creates a destination"
        );
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let evidence = target.join("editor-quality");
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::write(
        evidence.join("build-options-layout-latest.json"),
        ara_json::to_canonical_string(&json!({"cases":measurements,"exportExecuted":false})),
    )
    .unwrap();
}
