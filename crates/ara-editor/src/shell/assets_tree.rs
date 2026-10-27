//! Persisted folder navigation and splitter, independent of content undo.
use super::*;

impl Shell {
    pub(super) fn asset_tree(&mut self, editor: &Editor, out: &mut String, width: f32) {
        let folder = &editor.state.asset_folder;
        out.push_str(&format!("<scroll id='asset-folders' style='flex-basis:0px;flex-grow:{width};min-width:0px;min-height:0px;padding:4px;gap:0px;background:var(--panel-header)'><panel id='asset-folder-content' style='flex-shrink:0;gap:0px'>"));
        self.bindings.insert(
            ara_ui::source::stable_id("asset-folder-root"),
            Binding::Protocol("editor.workspace", json!({"folder":""})),
        );
        out.push_str(&format!("<button id='asset-folder-root' class='folder-row{}' label='Project' action='Dispatch'>{}<text id='asset-folder-root-label' text='Project' /></button>", if folder.is_empty(){" selected"}else{""},icons::icon("asset-folder-root","package-open")));
        let folders = folders(&self.browser.items);
        for path in &folders {
            if !visible(path, &editor.state.asset_expanded_folders) {
                continue;
            }
            let id = format!("asset-folder-{:x}", ara_ui::source::stable_id(path));
            let expanded = editor.state.asset_expanded_folders.contains(path);
            let children = folders
                .iter()
                .any(|p| p.strip_prefix(path).is_some_and(|p| p.starts_with('/')));
            let selected = path == folder;
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::AssetFolder(path.clone(), false),
            );
            self.bindings.insert(
                ara_ui::source::stable_id(&format!("{id}-arrow")),
                Binding::AssetFolder(path.clone(), true),
            );
            out.push_str(&format!("<panel id='{id}-row' class='row' style='flex-shrink:0;gap:0px;padding-left:{}px'><button id='{id}-arrow' class='ghost icon-button' style='width:18px;min-width:18px;padding:2px' label='{} {}' action='Dispatch'>{}</button><button id='{id}' class='folder-row{}' style='flex-grow:1;min-width:0px;padding-left:2px' label='{}' description='Double-click to expand or collapse; arrow keys navigate folders' action='Dispatch'>{}<text id='{id}-label' text='{}' /></button></panel>",path.matches('/').count()*12,if expanded{"Collapse"}else{"Expand"},escape(path),if children { icons::icon(&format!("{id}-arrow"),if expanded{"chevron-down"}else{"chevron-right"}) }else{String::new()},if selected{" selected"}else{""},escape(path),icons::icon(&id,if expanded{"folder-open"}else{"folder"}),escape(names::file_name(path))));
        }
        out.push_str("</panel></scroll><splitter id='asset-sidebar-splitter' cursor='ew-resize' class='splitter' style='width:5px;min-width:5px;flex-shrink:0' label='Resize folders' action='ResizeAssets' />");
    }

    pub(super) fn asset_folder_action(
        &mut self,
        editor: &mut Editor,
        path: &str,
        arrow: bool,
    ) -> ara_ui::Result<()> {
        let now = std::time::Instant::now();
        let double = self
            .asset_folder_click
            .as_ref()
            .is_some_and(|(previous, at)| {
                previous == path && now.duration_since(*at).as_millis() < 400
            });
        if arrow || double {
            editor.action("editor.workspace", json!({"toggleFolder":path}))?;
            self.asset_folder_click = None;
        } else {
            self.asset_folder_click = Some((path.to_owned(), now));
        }
        if arrow {
            return Ok(());
        }
        editor.action("editor.workspace", json!({"folder":path}))?;
        Ok(())
    }

    pub(super) fn asset_folder_key(
        &mut self,
        editor: &mut Editor,
        key: i32,
    ) -> ara_ui::Result<bool> {
        let Some(Binding::AssetFolder(path, _)) =
            self.input.focused().and_then(|id| self.bindings.get(&id))
        else {
            return Ok(false);
        };
        let path = path.clone();
        if !matches!(key, 79..=82) {
            return Ok(false);
        }
        self.asset_folder_click = None;
        if matches!(key, 79 | 80) {
            editor.action(
                "editor.workspace",
                json!({"toggleFolder":path,"expandFolder":key==79}),
            )?;
        } else {
            let folders = folders(&self.browser.items)
                .into_iter()
                .filter(|p| visible(p, &editor.state.asset_expanded_folders))
                .collect::<Vec<_>>();
            if let Some(at) = folders.iter().position(|p| *p == path) {
                let next = if key == 81 {
                    (at + 1).min(folders.len() - 1)
                } else {
                    at.saturating_sub(1)
                };
                editor.action("editor.workspace", json!({"folder":folders[next]}))?;
                self.rebuild(editor)?;
                self.input.focus(
                    &mut self.ui,
                    ara_ui::source::stable_id(&format!(
                        "asset-folder-{:x}",
                        ara_ui::source::stable_id(&folders[next])
                    )),
                )?;
            }
        }
        self.rebuild(editor)?;
        Ok(true)
    }
}

fn folders(items: &[assets::Asset]) -> std::collections::BTreeSet<String> {
    let mut folders = std::collections::BTreeSet::new();
    for asset in items {
        let mut path = if asset.kind == "folder" {
            asset.path.as_str()
        } else {
            asset.path.rsplit_once('/').map_or("", |(p, _)| p)
        };
        while !path.is_empty() {
            folders.insert(path.to_owned());
            path = path.rsplit_once('/').map_or("", |(p, _)| p);
        }
    }
    folders
}
fn visible(path: &str, expanded: &std::collections::BTreeSet<String>) -> bool {
    path.match_indices('/')
        .all(|(at, _)| expanded.contains(&path[..at]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descendants_start_closed_and_respect_every_ancestor() {
        let mut expanded = std::collections::BTreeSet::new();
        assert!(visible("Assets", &expanded));
        assert!(!visible("Assets/Art", &expanded));
        expanded.insert("Assets/Art".into());
        assert!(!visible("Assets/Art/Models", &expanded));
        expanded.insert("Assets".into());
        assert!(visible("Assets/Art/Models", &expanded));
    }
}
