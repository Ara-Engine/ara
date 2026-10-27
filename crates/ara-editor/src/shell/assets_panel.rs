//! Folder projection, bounded thumbnail cards and import/dependency inspection.
//!
//! Tiles follow UE's Content Browser and Unity's Project window: one box per asset (so
//! rows align), a 64 px thumbnail and the name without its extension centred under it
//! on at most two lines; the type shows as the icon (no thumbnail) or a corner badge,
//! and the full file name in the tooltip. Files come in by drag and drop or the
//! context menu's Import.
use super::*;
#[path = "assets_panel/types.rs"]
mod types;
impl Shell {
    /// A digest of everything [`assets_panel`](Self::assets_panel) reads: the files,
    /// the folder, search, view mode and selection, density, thumbnails, import progress,
    /// the content's size and scroll, Show All and the keymap.
    pub(super) fn assets_key(&self, editor: &Editor) -> u64 {
        let extent = |name| {
            self.ui.find(name).map(|i| {
                let rect = self.ui.nodes[i].rect;
                [rect.w.to_bits(), rect.h.to_bits()]
            })
        };
        let content = (extent("assets-content"), extent("asset-grid"));
        memo::digest((
            (
                self.browser.revision,
                self.asset_ops.revision,
                self.asset_import.revision,
            ),
            (
                &editor.state.asset_folder,
                editor.state.asset_sidebar_width.to_bits(),
                &editor.state.asset_expanded_folders,
            ),
            editor.state.searches.get("assets"),
            editor.state.asset_grid,
            (
                &editor.state.asset_path,
                self.asset_selection.selected(),
                self.asset_rename.as_ref().map(|r| &r.path),
            ),
            editor.state.comfortable(),
            (self.thumbnail_revision, self.imports.revision),
            content,
            self.ui.size.map(f32::to_bits),
            self.asset_offset.to_bits(),
            self.assets_show_all,
            self.keymap_revision,
        ))
    }
    pub(super) fn assets_panel(&mut self, editor: &Editor, out: &mut String) {
        self.asset_selection_order(editor);
        // Folder column and breadcrumb hide in narrow docks (see `narrow`).
        out.push_str("<panel id='asset-toolbar' class='toolbar'>");
        self.bindings.insert(
            ara_ui::source::stable_id("asset-create"),
            Binding::ContextMenu(commands::menus::Context::Assets, commands::Target::None),
        );
        out.push_str(&format!("<button id='asset-create' class='ghost' style='padding-left:4px;padding-right:2px' label='Create' description='Create folders, C# scripts, materials, scenes and prefabs' action='Dispatch'>{}{}</button>",icons::icon("asset-create","plus"),icons::icon_in("asset-create-more","chevron-down","icon-sm")));
        let content = self
            .ui
            .find("assets-content")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(self.ui.size[0]);
        // Narrow docks drop the breadcrumb and the folder column.
        let narrow = content < 420.;
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
        let side = if narrow {
            0.
        } else {
            editor.state.asset_sidebar_width.min(content * 0.6).max(96.)
        };
        out.push_str("</panel>");
        self.asset_operations_markup(out);
        self.asset_import_markup(out);
        if let Some(problem) = &self.browser.error {
            out.push_str(&format!("<text id='assets-index-error' class='caption' style='white-space:normal' text='{}' />",escape(problem)));
        }
        self.asset_types_markup(editor, out);
        out.push_str("<panel id='assets-content' style='flex-direction:row;flex-grow:1;flex-basis:0px;min-height:0px'>");
        if side > 0. {
            self.asset_tree(editor, out, side / (content - side - 5.).max(1.));
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
                let parent = if asset.kind == "folder" {
                    asset.path.as_str()
                } else {
                    asset.path.rsplit_once('/').map_or("", |(p, _)| p)
                };
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
            items.extend(subfolders.into_iter().map(|path| {
                self.browser
                    .items
                    .iter()
                    .find(|asset| asset.path == path && asset.kind == "folder")
                    .cloned()
                    .unwrap_or_else(|| {
                        assets::Asset::folder(
                            path.clone(),
                            self.browser.authored_folder(&path).ok(),
                        )
                    })
            }));
        }
        items.extend(
            self.browser
                .items
                .iter()
                .filter(|a| {
                    a.kind != "folder"
                        && (self.assets_show_all || !is_noise(a))
                        && matches_query(a, query)
                        && (searching || in_folder(&a.path))
                })
                .cloned(),
        );
        self.asset_rows.items.clear();
        self.asset_entries.clear();
        for asset in &items {
            let key = ara_ui::source::stable_id(&asset.path);
            // Searches span folders: the list says where each result lives.
            let detail = if searching {
                asset
                    .path
                    .rsplit_once('/')
                    .map_or("Project", |(f, _)| f)
                    .to_owned()
            } else {
                names::kind_label(asset).to_owned()
            };
            let name = names::stem(names::file_name(&asset.path)).to_owned();
            self.asset_rows.items.push((key, name, detail));
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
                    "Drop files here from your file manager, or right-click to import."
                } else {
                    "Try type:mesh, type:texture or type:material."
                },
                &[(
                    "assets-empty-material",
                    "New material",
                    "assets.create.material",
                )],
            );
            out.push_str("</panel>");
            self.asset_footer(out, items.len(), editor);
            return;
        }
        if !editor.state.asset_grid {
            // Names (and searched folders) end in "…" at their column, not mid-glyph:
            // the same columns `Shell::mount_rows` gives the row pool.
            let width = self
                .ui
                .find("asset-rows")
                .map_or(content - side, |i| self.ui.nodes[i].rect.w)
                .max(120.)
                - 8.;
            let detail = names::detail_width(width);
            for (_, name, folder) in &mut self.asset_rows.items {
                *name = self
                    .name_fit
                    .lines(&mut self.text, name, width - detail - 34., 12., 1)[0]
                    .clone();
                if searching {
                    *folder = self
                        .name_fit
                        .lines(&mut self.text, folder, detail - 10., 11., 1)[0]
                        .clone();
                }
            }
            // The row height is in the markup: a density change remounts the row pool.
            let row = if editor.state.comfortable() { 26 } else { 22 };
            out.push_str(&format!(
                "<scroll id='asset-rows' class='body asset-list rows-{row}' style='min-height:0px;min-width:0px;flex-grow:1;flex-basis:0px' /></panel>"
            ));
            self.asset_footer(out, items.len(), editor);
            return;
        }
        let width = self
            .ui
            .find("asset-grid")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(self.ui.size[0] - side - if side > 0. { 5. } else { 0. });
        let height = self
            .ui
            .find("assets-content")
            .map(|i| self.ui.nodes[i].rect.h)
            .unwrap_or(190.);
        self.asset_grid_size = [width, height];
        // Short drawers put the name beside a smaller thumbnail.
        let compact = height < TILE[1] + 16.;
        let ([card_width, card_height], thumbnail) = if compact {
            (SIDE_TILE, 48.)
        } else {
            (TILE, 64.)
        };
        let column_step = card_width + GAP;
        let row_step = card_height + GAP;
        let available = (width - 12.).max(1.);
        let columns = ((available - GAP) / column_step).floor().max(1.) as usize;
        // The columns are centred: the space a partial column would take is shared by
        // both sides instead of piling up on the right.
        let left = ((available - columns as f32 * column_step + GAP) * 0.5).max(GAP);
        self.asset_grid_extent = items.len().div_ceil(columns) as f32 * row_step;
        if let Some(path) = self
            .asset_rename
            .as_ref()
            .map(|rename| rename.path.as_str())
            .or_else(|| self.asset_ops.rename_path())
            && let Some(at) = items.iter().position(|asset| asset.path == path)
        {
            let top = (at / columns) as f32 * row_step;
            if top < self.asset_offset || top + card_height > self.asset_offset + height {
                self.asset_offset = top;
            }
        }
        self.asset_offset = self
            .asset_offset
            .clamp(0., (self.asset_grid_extent - height).max(0.));
        let first_row = (self.asset_offset / row_step).floor() as usize;
        let first = first_row * columns;
        let count = ((height / row_step).ceil() as usize + 2) * columns;
        // Cards sit at non-negative offsets from the first visible row; the partial-row
        // scroll is a translation (ARA4014 rejects negative lengths, FINDINGS #18).
        let scroll = self.asset_offset - first_row as f32 * row_step;
        out.push_str(&format!("<scroll id='asset-grid' style='min-width:0px;flex-grow:1;flex-basis:0px;overflow:hidden'><panel id='asset-grid-rows' style='position:absolute;left:0px;top:0px;width:100%;height:100%;translate-y:-{scroll}px'>"));
        // Where the thumbnail sits in its card (badges hang off its corners).
        let thumb = if compact {
            [8., (card_height - thumbnail) * 0.5]
        } else {
            [(card_width - thumbnail) * 0.5, 6.]
        };
        let caption = if compact {
            card_width - thumbnail - 24.
        } else {
            card_width - 12.
        };
        for (n, asset) in items.iter().enumerate().skip(first).take(count) {
            let id = format!("asset-card-{n}");
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::Asset(asset.clone()),
            );
            let selected = self
                .asset_selection
                .selected()
                .contains(&ara_ui::source::stable_id(&asset.path));
            let file = names::file_name(&asset.path);
            // Importing or failed: the tooltip's note, the badge's icon, tint and reason.
            let import = self.imports.of(asset).map(|import| match import {
                imports::Import::Pending => (
                    " · importing",
                    "loader-circle",
                    "accent",
                    "Importing".to_owned(),
                ),
                imports::Import::Failed(error) => (
                    " · import failed",
                    "triangle-alert",
                    "warning",
                    error.to_owned(),
                ),
            });
            let state = import.as_ref().map_or("", |i| i.0);
            // The tooltip carries what the tile leaves out: the extension and the type.
            let tip = format!("{file} · {}{state}", names::kind_label(asset));
            out.push_str(&format!("<button id='{id}' class='{}' style='position:absolute;left:{}px;top:{}px;width:{card_width}px;height:{card_height}px' label='{}' description='{}' action='Dispatch'>",match (compact,selected){(true,true)=>"asset-card asset-card-side asset-selected",(true,false)=>"asset-card asset-card-side",(false,true)=>"asset-card asset-selected",(false,false)=>"asset-card"},left+(n%columns) as f32*column_step,GAP+(n/columns-first_row) as f32*row_step,escape(names::stem(file)),escape(&tip)));
            let shown = self.asset_thumbnail(asset, out, &id, thumbnail);
            // A thumbnail shows the asset, not its type: a corner badge does.
            if shown {
                out.push_str(&format!("<panel id='{id}-type' class='type-badge' style='left:{}px;top:{}px'>{}</panel>",thumb[0]+thumbnail-14.,thumb[1]+thumbnail-14.,icons::icon_in(&format!("{id}-type"),asset_icon(asset),&format!("icon-xs {}",names::kind_color(asset)))));
            }
            // Importing and failed assets carry a badge on the thumbnail's top corner.
            if let Some((_, icon, tint, why)) = &import {
                out.push_str(&format!("<panel id='{id}-import' class='import-badge' style='left:{}px;top:{}px' description='{}'>{}</panel>",thumb[0]+thumbnail-14.,thumb[1]-4.,escape(why),icons::icon_in(&format!("{id}-import"),icon,&format!("icon-sm {tint}"))));
            }
            let lines = self
                .name_fit
                .lines(&mut self.text, names::stem(file), caption, 11., 2)
                .to_vec();
            out.push_str(&format!(
                "<panel id='{id}-label' class='{}'>",
                if compact {
                    "asset-label-side"
                } else {
                    "asset-label"
                }
            ));
            // `{card}-name` is the first line (what tests and routes look up).
            for (l, line) in lines.iter().enumerate() {
                let line_id = if l == 0 {
                    format!("{id}-name")
                } else {
                    format!("{id}-name-{}", l + 1)
                };
                out.push_str(&format!(
                    "<text id='{line_id}' class='asset-name {}' text='{}' />",
                    if selected { "on-selection" } else { "fg" },
                    escape(line)
                ));
            }
            out.push_str("</panel></button>");
        }
        out.push_str("</panel></scroll></panel>");
        self.asset_footer(out, items.len(), editor);
    }
    fn asset_footer(&mut self, out: &mut String, count: usize, editor: &Editor) {
        let selected = self
            .asset_selection
            .selected()
            .len()
            .max(usize::from(editor.state.asset_path.is_some()));
        out.push_str(&format!("<panel id='assets-footer' class='footer'><text id='assets-count' text='{count} item{}{}' /></panel>",if count==1{""}else{"s"},if selected>0 {format!(" · {selected} selected")}else{String::new()}));
    }
    /// The asset's thumbnail, or its type icon on a quiet square until one exists (or
    /// when it has none); true when the thumbnail image is shown.
    pub(super) fn asset_thumbnail(
        &mut self,
        asset: &assets::Asset,
        out: &mut String,
        id: &str,
        size: f32,
    ) -> bool {
        if let Some(slot) = self.thumbnail_slots.get(&asset.path) {
            out.push_str(&format!(
                "<image id='{id}-thumbnail' src='{}' style='width:{size}px;height:{size}px' />",
                crate::asset_tools::thumbnails::image_name(*slot)
            ));
            true
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
            out.push_str(&format!("<panel id='{id}-placeholder' class='asset-placeholder' style='width:{size}px;height:{size}px'>{}</panel>",icons::icon_in(id,asset_icon(asset),&format!("icon-lg {}",names::kind_color(asset)))));
            false
        }
    }
}
impl Shell {
    /// The folder holding the most model files (a first look at a project's art).
    pub(crate) fn model_folder(&self) -> Option<String> {
        let mut counts = std::collections::BTreeMap::new();
        for asset in self
            .browser
            .items
            .iter()
            .filter(|a| matches!(a.kind.as_str(), "gltf" | "glb" | "fbx"))
        {
            let folder = asset.path.rsplit_once('/').map_or("", |(f, _)| f);
            *counts.entry(folder.to_owned()).or_insert(0) += 1;
        }
        counts.into_iter().max_by_key(|(_, n)| *n).map(|(f, _)| f)
    }
    /// A search a user might type for one of the project's models: its first word.
    pub(crate) fn model_query(&self) -> Option<String> {
        let model = self
            .browser
            .items
            .iter()
            .find(|a| matches!(a.kind.as_str(), "gltf" | "glb" | "fbx"))?;
        let stem = model.name().split(['.', '_', '-']).next()?;
        Some(stem.to_lowercase())
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
/// Authored support files hide by default. Semantic documents and project-setting
/// entries remain reachable regardless of their storage extension.
pub(super) fn is_noise(asset: &assets::Asset) -> bool {
    if asset.document_kind.is_some() || asset.authored_path.is_none() {
        return false;
    }
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
/// Grid tile size: a 64 px thumbnail and two caption lines; and the short-drawer tile
/// (48 px thumbnail, the name beside it). Tiles are `GAP` apart.
const TILE: [f32; 2] = [96., 116.];
const SIDE_TILE: [f32; 2] = [184., 64.];
const GAP: f32 = 8.;
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
            let binding = Binding::Asset(asset.clone());
            self.bindings.insert(view.identity(key), binding);
        }
    }
}
