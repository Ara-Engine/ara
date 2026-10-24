//! Folder projection, bounded thumbnail cards and import/dependency inspection.
use super::*;
impl Shell {
    /// A digest of everything [`assets_panel`](Self::assets_panel) reads: the files,
    /// the folder, search, view mode and selection, density, thumbnails, the content's
    /// size and scroll, Show All and the keymap (the Import shortcut).
    pub(super) fn assets_key(&self, editor: &Editor) -> u64 {
        let content = self.ui.find("assets-content").map(|i| {
            let rect = self.ui.nodes[i].rect;
            [rect.w.to_bits(), rect.h.to_bits()]
        });
        memo::digest((
            self.browser.revision,
            &editor.state.asset_folder,
            editor.state.searches.get("assets"),
            editor.state.asset_grid,
            &editor.state.asset_path,
            editor.state.comfortable(),
            self.thumbnail_revision,
            content,
            self.ui.size.map(f32::to_bits),
            self.asset_offset.to_bits(),
            self.assets_show_all,
            self.keymap_revision,
        ))
    }
    pub(super) fn assets_panel(&mut self, editor: &Editor, out: &mut String) {
        // Folder column and breadcrumb hide in narrow docks (see `narrow`).
        out.push_str("<panel id='asset-toolbar' class='toolbar'>");
        self.bindings.insert(
            ara_ui::source::stable_id("asset-create"),
            Binding::ContextMenu(commands::menus::Context::Assets, commands::Target::None),
        );
        out.push_str(&format!("<button id='asset-create' class='ghost' style='padding-left:4px;padding-right:2px' label='Create' description='Create folders, materials, effects and controllers' action='Dispatch'>{}{}</button>",icons::icon("asset-create","plus"),icons::icon_in("asset-create-more","chevron-down","icon-sm")));
        let content = self
            .ui
            .find("assets-content")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(self.ui.size[0]);
        // Narrow docks show Import as an icon and drop the breadcrumb and folder column.
        let narrow = content < 420.;
        if narrow {
            self.tool_icon(
                out,
                "asset-import",
                "assets.import",
                "file-plus",
                editor,
                false,
            );
        } else {
            self.tool_text(
                out,
                "asset-import",
                "assets.import",
                "file-plus",
                "Import",
                "ghost",
            );
        }
        let folder = editor.state.asset_folder.clone();
        if !narrow {
            self.bindings.insert(
                ara_ui::source::stable_id("asset-crumb-root"),
                Binding::Protocol("editor.workspace", json!({"folder":""})),
            );
            out.push_str("<panel id='asset-crumbs' class='row' style='gap:2px;padding-left:4px;overflow:hidden;flex-shrink:1'><button id='asset-crumb-root' class='ghost' label='Project' action='Dispatch'><text id='asset-crumb-root-label' text='Project' /></button>");
            let mut path = String::new();
            for (n, part) in folder.split('/').filter(|p| !p.is_empty()).enumerate() {
                if !path.is_empty() {
                    path.push('/');
                }
                path.push_str(part);
                let id = format!("asset-crumb-{n}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Protocol("editor.workspace", json!({"folder":path})),
                );
                out.push_str(&format!(
                    "{}<button id='{id}' class='ghost' label='{}' action='Dispatch'><text id='{id}-label' text='{}' /></button>",
                    icons::icon(&format!("{id}-sep"), "chevron-right"),
                    escape(part),
                    escape(part)
                ));
            }
            out.push_str("</panel>");
        }
        self.search(out, "assets", editor);
        for (id, label, icon, grid) in [
            ("assets-grid", "Grid", "layout-grid", true),
            ("assets-list", "List", "list-tree", false),
        ] {
            self.bindings.insert(
                ara_ui::source::stable_id(id),
                Binding::Protocol("editor.workspace", json!({"grid":grid})),
            );
            let on = editor.state.asset_grid == grid;
            out.push_str(&format!("<button id='{id}' class='{} icon-button' label='{label}' description='{label} view' action='Dispatch'>{}</button>",if on{"selected"}else{"ghost"},if on{icons::icon_in(id,icon,"on-selection")}else{icons::icon(id,icon)}));
        }
        let side = if content < 420. {
            0.
        } else if content < 720. {
            140.
        } else {
            168.
        };
        out.push_str("</panel>");
        if !narrow {
            let current = editor
                .state
                .searches
                .get("assets")
                .cloned()
                .unwrap_or_default();
            out.push_str("<panel id='asset-types' class='type-chips'>");
            self.bindings.insert(
                ara_ui::source::stable_id("asset-type-all"),
                Binding::Protocol("editor.search", json!({"panel":"assets","query":""})),
            );
            let any = TYPE_CHIPS
                .iter()
                .any(|(_, t)| current.contains(&format!("type:{t}")));
            out.push_str(&format!("<button id='asset-type-all' class='type-chip{}' label='All types' action='Dispatch'><text id='asset-type-all-label' text='All' /></button>",if any{""}else{" selected"}));
            for (label, kind) in TYPE_CHIPS {
                let id = format!("asset-type-{kind}");
                let on = current.contains(&format!("type:{kind}"));
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Protocol("editor.search", json!({"panel":"assets","query": if on { String::new() } else { format!("type:{kind}") }})),
                );
                out.push_str(&format!("<button id='{id}' class='type-chip{}' label='{label}' description='Show only {}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",if on{" selected"}else{""},label.to_lowercase()));
            }
            out.push_str("</panel>");
        }
        out.push_str("<panel id='assets-content' style='flex-direction:row;flex-grow:1;flex-basis:0px;min-height:0px'>");
        if side > 0. {
            out.push_str(&format!("<scroll id='asset-folders' style='width:{side}px;min-width:{side}px;flex-basis:{side}px;flex-shrink:0;padding:4px;gap:0px;background:var(--panel-header)'>"));
            self.bindings.insert(
                ara_ui::source::stable_id("asset-folder-root"),
                Binding::Protocol("editor.workspace", json!({"folder":""})),
            );
            out.push_str(&format!("<button id='asset-folder-root' class='folder-row{}' label='Project' action='Dispatch'>{}<text id='asset-folder-root-label' text='Project' /></button>",if folder.is_empty(){" selected"}else{""},icons::icon("asset-folder-root","package-open")));
            let mut folders = std::collections::BTreeSet::new();
            for asset in &self.browser.items {
                let mut parts = asset.path.split('/').collect::<Vec<_>>();
                parts.pop();
                for n in 1..=parts.len() {
                    folders.insert(parts[..n].join("/"));
                }
            }
            for (n, path) in folders.iter().enumerate() {
                let depth = path.matches('/').count();
                let id = format!("asset-folder-{n}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&id),
                    Binding::Protocol("editor.workspace", json!({"folder":path})),
                );
                let on = *path == folder;
                out.push_str(&format!("<button id='{id}' class='folder-row{}' style='padding-left:{}px' label='{}' action='Dispatch'>{}<text id='{id}-label' text='{}' /></button>",if on{" selected"}else{""},8+12*(depth+1),escape(path),if on{icons::icon_in(&id,"folder-open","on-selection")}else{icons::icon(&id,"folder")},escape(path.rsplit('/').next().unwrap_or(path))));
            }
            out.push_str("</scroll>");
        }
        let query = editor
            .state
            .searches
            .get("assets")
            .map(String::as_str)
            .unwrap_or("");
        // The current folder's subfolders, then its files; a search looks everywhere.
        let in_folder = |path: &str| {
            let rest = if editor.state.asset_folder.is_empty() {
                Some(path)
            } else {
                path.strip_prefix(&format!("{}/", editor.state.asset_folder))
            };
            rest.is_some_and(|s| !s.contains('/'))
        };
        let searching = !query.trim().is_empty();
        let mut items: Vec<assets::Asset> = Vec::new();
        if !searching {
            let mut subfolders = std::collections::BTreeSet::new();
            for asset in &self.browser.items {
                if !self.assets_show_all && is_noise(asset) {
                    continue;
                }
                let parent = asset.path.rsplit_once('/').map_or("", |(p, _)| p);
                let mut path = parent;
                // Every ancestor of a visible file that is directly in this folder.
                while !path.is_empty() {
                    if in_folder(path) {
                        subfolders.insert(path.to_owned());
                        break;
                    }
                    path = path.rsplit_once('/').map_or("", |(p, _)| p);
                }
            }
            items.extend(subfolders.into_iter().map(|path| assets::Asset {
                path,
                reference: None,
                kind: "folder".into(),
                domain: None,
                uses: Vec::new(),
                stamp: 0,
            }));
        }
        items.extend(
            self.browser
                .items
                .iter()
                .filter(|a| {
                    (self.assets_show_all || !is_noise(a))
                        && matches_query(a, query)
                        && (searching || in_folder(&a.path))
                })
                .cloned(),
        );
        self.asset_rows.items.clear();
        self.asset_entries.clear();
        for asset in &items {
            let key = ara_ui::source::stable_id(&asset.path);
            self.asset_rows.items.push((key, asset.path.clone()));
            self.asset_entries.insert(key, asset.clone());
        }
        if items.is_empty() {
            self.empty(
                out,
                "assets-empty",
                "inbox",
                if query.is_empty() {
                    "This folder is empty"
                } else {
                    "No matching assets"
                },
                if query.is_empty() {
                    "Import files, or drop them into this folder from your file manager."
                } else {
                    "Try type:mesh, type:texture or type:material."
                },
                &[
                    ("assets-empty-import", "Import…", "assets.import"),
                    (
                        "assets-empty-material",
                        "New material",
                        "assets.create.material",
                    ),
                ],
            );
            out.push_str("</panel>");
            self.asset_footer(out, items.len(), editor);
            return;
        }
        if !editor.state.asset_grid {
            // The row height is in the markup: a density change remounts the row pool.
            let row = if editor.state.comfortable() { 26 } else { 22 };
            out.push_str(&format!(
                "<scroll id='asset-rows' class='body asset-list rows-{row}' /></panel>"
            ));
            self.asset_footer(out, items.len(), editor);
            return;
        }
        let width = self
            .ui
            .find("assets-content")
            .map(|i| self.ui.nodes[i].rect.w - side)
            .unwrap_or(self.ui.size[0] - side);
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
            (92., 104., 64.)
        };
        let column_step = card_width + 8.;
        let row_step = card_height + 8.;
        let columns = ((width - 16.) / column_step).floor().max(1.) as usize;
        self.asset_grid_extent = items.len().div_ceil(columns) as f32 * row_step;
        self.asset_offset = self
            .asset_offset
            .clamp(0., (self.asset_grid_extent - height).max(0.));
        let first_row = (self.asset_offset / row_step).floor() as usize;
        let first = first_row * columns;
        let count = ((height / row_step).ceil() as usize + 2) * columns;
        // Cards sit at non-negative offsets from the first visible row; the partial-row
        // scroll is a translation (ARA4014 rejects negative lengths, FINDINGS #18).
        let scroll = self.asset_offset - first_row as f32 * row_step;
        out.push_str(&format!("<panel id='asset-grid' style='flex-grow:1;flex-basis:0px;overflow:hidden'><panel id='asset-grid-rows' style='position:absolute;left:0px;top:0px;width:100%;height:100%;translate-y:-{scroll}px'>"));
        for (n, asset) in items.iter().enumerate().skip(first).take(count) {
            let id = format!("asset-card-{n}");
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                if asset.kind == "folder" {
                    Binding::Protocol("editor.workspace", json!({"folder": asset.path}))
                } else {
                    Binding::Asset(asset.clone())
                },
            );
            out.push_str(&format!("<button id='{id}' class='asset-card {}' style='position:absolute;left:{}px;top:{}px;width:{card_width}px;height:{card_height}px;flex-direction:{}' label='{}' description='{}' action='Dispatch'>",if editor.state.asset_path.as_deref()==Some(&asset.path){"asset-selected"}else{""},8.+(n%columns) as f32*column_step,8.+(n/columns-first_row) as f32*row_step,if compact{"row"}else{"column"},escape(&asset.path),escape(&asset.path)));
            self.asset_thumbnail(asset, out, &id, thumbnail);
            if compact {
                out.push_str(&format!("<panel id='{id}-caption' style='flex-grow:1;flex-basis:0px;min-width:0px;gap:3px'>"));
            }
            let color = match asset.kind.as_str() {
                "glb" | "gltf" | "fbx" => "chart-1",
                "png" | "jpg" | "jpeg" | "hdr" | "exr" => "chart-2",
                "graph" | "material" => match asset.domain.as_deref() {
                    Some("vfx") => "chart-5",
                    _ => "chart-3",
                },
                "scene" | "prefab" => "chart-4",
                "animation" => "chart-6",
                _ => "muted-foreground",
            };
            out.push_str(&format!("<panel id='{id}-bar' class='type-bar' style='width:{}px;background:var(--{color})' />",if compact{40.}else{thumbnail}));
            let selected = editor.state.asset_path.as_deref() == Some(&asset.path);
            out.push_str(&format!("<text id='{id}-name' style='width:{}px;font-size:11px;overflow:hidden' class='{}' text='{}' /><text id='{id}-kind' class='{}' style='font-size:10px' text='{}' />",card_width-8.,if selected{"on-selection"}else{"fg"},escape(asset.path.rsplit('/').next().unwrap_or(&asset.path)),if selected{"on-selection"}else{"caption"},escape(asset.domain.as_deref().unwrap_or(&asset.kind))));
            if compact {
                out.push_str("</panel>");
            }
            out.push_str("</button>");
        }
        out.push_str("</panel></panel></panel>");
        self.asset_footer(out, items.len(), editor);
    }
    fn asset_footer(&mut self, out: &mut String, count: usize, editor: &Editor) {
        out.push_str(&format!("<panel id='assets-footer' class='footer'><text id='assets-count' text='{count} item{}{}' /></panel>",if count==1{""}else{"s"},if editor.state.asset_path.is_some(){" · 1 selected"}else{""}));
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
            out.push_str(&format!("<panel id='{id}-placeholder' style='width:{size}px;height:{size}px;align-items:center;justify-content:center;background:var(--background);border-radius:var(--radius-sm)'>{}</panel>",icons::icon_in(id,asset_icon(asset),"icon-lg")));
        }
    }
}
/// One icon per asset concept.
pub(super) fn asset_icon(asset: &assets::Asset) -> &'static str {
    match asset.kind.as_str() {
        "png" | "jpg" | "jpeg" | "hdr" | "exr" => "image",
        "glb" | "gltf" | "fbx" => "package",
        "scene" => "clapperboard",
        "prefab" => "box",
        "animation" => "person-standing",
        "graph" | "material" => match asset.domain.as_deref() {
            Some("vfx") => "sparkles",
            Some("material") => "contrast",
            _ => "layers",
        },
        "cs" => "file-code",
        "json" => "file-code",
        "folder" => "folder",
        "wav" | "ogg" | "mp3" | "flac" => "volume-2",
        _ => "file",
    }
}
/// Files that are not assets: glTF buffers, notes, project and IDE files. Hidden
/// unless Assets > Show All Files is on.
pub(super) fn is_noise(asset: &assets::Asset) -> bool {
    matches!(
        asset.kind.as_str(),
        "bin"
            | "md"
            | "toml"
            | "json"
            | "csproj"
            | "sln"
            | "slnx"
            | "lock"
            | "txt"
            | "log"
            | "tmp"
            | "gitignore"
            | "gitattributes"
    )
}
/// The type chips above the asset grid: label and the `type:` query each sets.
pub(super) const TYPE_CHIPS: &[(&str, &str)] = &[
    ("Models", "mesh"),
    ("Textures", "texture"),
    ("Materials", "material"),
    ("Prefabs", "prefab"),
    ("Scenes", "scene"),
    ("Audio", "audio"),
    ("Scripts", "script"),
    ("VFX", "vfx"),
];
fn matches_query(asset: &assets::Asset, query: &str) -> bool {
    query.split_whitespace().all(|term| {
        if let Some(kind) = term.strip_prefix("type:") {
            match kind {
                "mesh" => matches!(asset.kind.as_str(), "gltf" | "glb" | "fbx"),
                "texture" => matches!(
                    asset.kind.as_str(),
                    "png" | "jpg" | "jpeg" | "hdr" | "exr" | "ktx2"
                ),
                "audio" => matches!(asset.kind.as_str(), "wav" | "ogg" | "mp3" | "flac"),
                "script" => asset.kind == "cs",
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
impl Shell {
    /// Bind the asset list rows on screen: a folder opens, a file is an asset. Only
    /// visible rows can be clicked, so rebuilds and scrolls bind just these.
    pub(super) fn bind_asset_rows(&mut self) {
        let Some(view) = &self.asset_view else {
            return;
        };
        for key in view.visible_keys() {
            let Some(asset) = self.asset_entries.get(&key) else {
                continue;
            };
            let binding = if asset.kind == "folder" {
                Binding::Protocol("editor.workspace", json!({"folder": asset.path}))
            } else {
                Binding::Asset(asset.clone())
            };
            self.bindings.insert(view.identity(key), binding);
        }
    }
}
