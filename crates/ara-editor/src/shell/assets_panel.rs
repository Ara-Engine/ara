//! Folder projection, bounded thumbnail cards and import/dependency inspection.
use super::*;
impl Shell {
    pub(super) fn assets_panel(&mut self, editor: &Editor, out: &mut String) {
        out.push_str("<panel id='asset-toolbar' class='toolbar'>");
        self.search(out, "assets", editor);
        for (id, label, grid) in [
            ("assets-grid", "Grid", true),
            ("assets-list", "List", false),
        ] {
            out.push_str(&self.bind_button(
                id,
                label,
                Binding::Protocol("editor.workspace", json!({"grid":grid})),
                if editor.state.asset_grid == grid {
                    "small selected"
                } else {
                    "small quiet"
                },
            ));
        }
        out.push_str("<text id='asset-hint' class='badge' text='type:mesh  type:texture  type:material' /></panel><panel id='assets-content' style='flex-direction:row;flex-grow:1;flex-basis:0px;min-height:0px'><scroll id='asset-folders' style='width:168px;min-width:168px;flex-basis:168px;flex-shrink:0;padding:8px;gap:2px;border-width:1px;border-color:var(--border)'>");
        out.push_str(&self.bind_button(
            "asset-folder-root",
            "Project",
            Binding::Protocol("editor.workspace", json!({"folder":""})),
            "small quiet folder-row",
        ));
        let mut folders = std::collections::BTreeSet::new();
        for asset in &self.browser.items {
            let mut parts = asset.path.split('/').collect::<Vec<_>>();
            parts.pop();
            for n in 1..=parts.len() {
                folders.insert(parts[..n].join("/"));
            }
        }
        for (n, path) in folders.iter().enumerate() {
            let label = format!(
                "{}{}",
                "  ".repeat(path.matches('/').count()),
                path.rsplit('/').next().unwrap_or(path)
            );
            out.push_str(&self.bind_button(
                &format!("asset-folder-{n}"),
                &label,
                Binding::Protocol("editor.workspace", json!({"folder":path})),
                if *path == editor.state.asset_folder {
                    "small selected folder-row"
                } else {
                    "small quiet folder-row"
                },
            ));
        }
        out.push_str("</scroll>");
        let query = editor
            .state
            .searches
            .get("assets")
            .map(String::as_str)
            .unwrap_or("");
        let items = self
            .browser
            .items
            .iter()
            .filter(|a| {
                matches_query(a, query)
                    && (editor.state.asset_folder.is_empty()
                        || a.path
                            .strip_prefix(&format!("{}/", editor.state.asset_folder))
                            .is_some_and(|s| !s.contains('/')))
            })
            .cloned()
            .collect::<Vec<_>>();
        self.asset_rows.items.clear();
        self.asset_entries.clear();
        for asset in &items {
            let key = ara_ui::source::stable_id(&asset.path);
            self.asset_rows.items.push((key, asset.path.clone()));
            self.asset_entries.insert(key, asset.clone());
        }
        if !editor.state.asset_grid {
            out.push_str("<scroll id='asset-rows' class='body asset-list' /></panel>");
            return;
        }
        let width = self
            .ui
            .find("assets-content")
            .map(|i| self.ui.nodes[i].rect.w - 168.)
            .unwrap_or(self.ui.size[0] - 168.);
        let height = self
            .ui
            .find("assets-content")
            .map(|i| self.ui.nodes[i].rect.h)
            .unwrap_or(190.);
        self.asset_grid_size = [width, height];
        // Short drawers keep filenames visible beside the thumbnail.
        let compact = height < 124.;
        let (card_width, card_height, thumbnail) = if compact {
            (168., 64., 48.)
        } else {
            (104., 108., 68.)
        };
        let column_step = card_width + 8.;
        let row_step = card_height + 8.;
        let columns = ((width - 16.) / column_step).floor().max(1.) as usize;
        self.asset_grid_extent = items.len().div_ceil(columns) as f32 * row_step;
        self.asset_offset = self
            .asset_offset
            .clamp(0., (self.asset_grid_extent - height).max(0.));
        let first = (self.asset_offset / row_step).floor() as usize * columns;
        let count = ((height / row_step).ceil() as usize + 2) * columns;
        out.push_str("<panel id='asset-grid' style='flex-grow:1;flex-basis:0px;overflow:hidden'>");
        for (n, asset) in items.iter().enumerate().skip(first).take(count) {
            let id = format!("asset-card-{n}");
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::Asset(asset.clone()),
            );
            out.push_str(&format!("<button id='{id}' class='asset-card {}' style='position:absolute;left:{}px;top:{}px;width:{card_width}px;height:{card_height}px;flex-direction:{}' label='{}' description='{}' action='Dispatch'>",if editor.state.asset_path.as_deref()==Some(&asset.path){"asset-selected"}else{""},8.+(n%columns) as f32*column_step,8.+(n/columns) as f32*row_step-self.asset_offset,if compact{"row"}else{"column"},escape(&asset.path),escape(&asset.path)));
            self.asset_thumbnail(asset, out, &id, thumbnail);
            if compact {
                out.push_str(&format!("<panel id='{id}-caption' style='flex-grow:1;flex-basis:0px;min-width:0px;gap:3px'>"));
            }
            out.push_str(&format!("<text id='{id}-name' style='width:92px;font-size:11px;overflow:hidden' text='{}' /><text id='{id}-kind' class='badge' text='{}' />",escape(asset.path.rsplit('/').next().unwrap_or(&asset.path)),escape(asset.domain.as_deref().unwrap_or(&asset.kind))));
            if compact {
                out.push_str("</panel>");
            }
            out.push_str("</button>");
        }
        out.push_str("</panel></panel>");
    }
    pub(super) fn asset_thumbnail(
        &mut self,
        asset: &assets::Asset,
        out: &mut String,
        id: &str,
        size: f32,
    ) {
        if let Some(slot) = self.thumbnail_slots.get(&asset.path) {
            out.push_str(&format!(
                "<image id='{id}-thumbnail' src='{}' style='width:{size}px;height:{size}px' />",
                crate::asset_tools::thumbnails::image_name(*slot)
            ));
        } else {
            if !asset.path.starts_with("live:")
                && matches!(
                    asset.kind.as_str(),
                    "png"
                        | "jpg"
                        | "jpeg"
                        | "hdr"
                        | "exr"
                        | "glb"
                        | "gltf"
                        | "fbx"
                        | "material"
                        | "graph"
                )
            {
                self.thumbnails.request(&asset.path);
            }
            out.push_str(&format!("<panel id='{id}-placeholder' style='width:{size}px;height:{size}px;align-items:center;justify-content:center;background:var(--background);border-radius:var(--radius-sm)'>{}</panel>",icons::icon(id,match asset.kind.as_str(){"png"|"jpg"=>"file-image","glb"|"gltf"|"fbx"=>"package","graph"|"material"=>"layers",_=>"file"})));
        }
    }
}
fn matches_query(asset: &assets::Asset, query: &str) -> bool {
    query.split_whitespace().all(|term| {
        if let Some(kind) = term.strip_prefix("type:") {
            match kind {
                "mesh" => matches!(asset.kind.as_str(), "gltf" | "glb" | "fbx"),
                "texture" => matches!(asset.kind.as_str(), "png" | "jpg" | "jpeg" | "hdr" | "exr"),
                "material" => {
                    matches!(asset.kind.as_str(), "material" | "materialinstance")
                        || asset.domain.as_deref() == Some("material")
                }
                "vfx" => asset.domain.as_deref() == Some("vfx"),
                _ => asset.kind.eq_ignore_ascii_case(kind),
            }
        } else {
            asset.path.to_lowercase().contains(&term.to_lowercase())
        }
    })
}
