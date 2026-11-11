//! The other panels: Assets (the project's files as a grid, folders first, breadcrumb and
//! search), Console (the diagnostics bus, newest last, severity toggles with counts) and
//! Problems (errors and warnings now, grouped by file). An unknown panel says so.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{Align, Axis, El, Justify, col, el, icon, row, text, virtual_list};

use super::Studio;
use super::state::LogRow;
use crate::assets::Asset;

pub(super) fn view(s: &Studio, panel: &str) -> El<Studio> {
    match panel {
        "assets" => assets(s),
        "console" => console(s),
        "problems" => problems(s),
        "mesh-paint" => super::mesh_paint::view(s),
        "agents" => super::agents::agents(s),
        "history" => super::agents::history(s),
        "lighting" => super::lighting::view(s),
        "profiler" => super::profiler::view(s),
        "build" => super::build_page::view(s),
        "material" => super::graph::view(s, "material"),
        "animation" => super::graph::view(s, "animation"),
        "vfx" => super::vfx::view(s),
        // Import settings live in the asset inspector; the Import tab shows the same.
        "import" => match s.editor.state.asset_path.as_deref() {
            Some(path) => super::asset_inspector::view(s, path),
            // No asset picked: the project's importer defaults.
            None => super::asset_inspector::defaults_view(s),
        },
        "settings" => col().grow(1.0).justify(Justify::Center).child(empty(
            &s.look,
            icons::SETTINGS,
            "Settings open in their own window",
            "Edit › Preferences (⌘,) for yours, Project Settings for the project's.",
            Some(
                button(
                    &s.look,
                    Tone::Secondary,
                    Size::Sm,
                    Some(icons::SETTINGS),
                    "Open Preferences",
                )
                .on_press(|s: &mut Studio| {
                    super::preferences::open(s, crate::settings::Scope::User)
                }),
            ),
        )),
        other => {
            let (title, glyph) = super::dock::panel_title(other);
            empty(
                &s.look,
                glyph,
                &format!("{title} isn't in this editor"),
                "Its layout names a panel this version doesn't have; close the tab.",
                None,
            )
        }
    }
}

/// How Assets lays out its items: a list, or tiles of three sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TileSize {
    List,
    Small,
    #[default]
    Medium,
    Large,
}

impl TileSize {
    /// A tile's width and its picture's size, design px.
    fn tile(self) -> (f32, f32) {
        match self {
            TileSize::List | TileSize::Small => (68.0, 40.0),
            TileSize::Medium => (84.0, 52.0),
            TileSize::Large => (120.0, 84.0),
        }
    }
}

/// Assets' window-only view state: layout, type filter, folder history, the Add menu.
#[derive(Clone, Debug, Default)]
pub struct AssetsView {
    pub size: TileSize,
    /// A type filter (`FILTERS`), showing that type across the whole project.
    pub filter: Option<&'static str>,
    pub back: Vec<String>,
    pub forward: Vec<String>,
    /// The Add menu is open.
    pub add: bool,
    /// Project files that are not assets show too (notes, JSON, solution files).
    pub show_all: bool,
    /// The view menu (⋯) is open.
    pub menu: bool,
    /// Move to Folder: the asset moving and the folder search typed.
    pub moving: Option<(String, String)>,
}

/// The type filters: id, label, icon.
const FILTERS: [(&str, &str, stillkit::icons::Icon); 9] = [
    ("model", "Models", icons::BOX),
    ("material", "Materials", icons::PALETTE),
    ("texture", "Textures", icons::IMAGE),
    ("audio", "Audio", icons::VOLUME_2),
    ("scene", "Scenes", icons::CLAPPERBOARD),
    ("prefab", "Prefabs", icons::PACKAGE),
    ("script", "Scripts", icons::FILE_CODE),
    ("vfx", "Effects", icons::SPARKLES),
    ("animation", "Animation", icons::PERSON_STANDING),
];

/// The type an asset files under (`FILTERS`), if any.
fn category(a: &Asset) -> Option<&'static str> {
    if a.document_kind.as_deref() == Some("scene") {
        return Some("scene");
    }
    match (a.domain.as_deref(), a.kind.as_str()) {
        (Some("material"), _) => Some("material"),
        (Some("vfx"), _) => Some("vfx"),
        (Some("animation"), _) | (_, "animation") => Some("animation"),
        (_, "gltf" | "glb" | "fbx" | "obj") => Some("model"),
        (_, "png" | "jpg" | "jpeg" | "tga" | "exr" | "hdr" | "ktx2" | "webp") => Some("texture"),
        (_, "wav" | "ogg" | "mp3" | "flac") => Some("audio"),
        (_, "prefab") => Some("prefab"),
        (_, "cs") => Some("script"),
        _ => None,
    }
}

/// A type's colour (the strip under its tiles, as Unreal's content browser has).
fn category_tint(l: &ara_kit::look::Look, category: &str) -> stillkit::Rgba {
    let c = &l.c;
    match category {
        "model" => c.chart_1.0,
        "material" => c.chart_2.0,
        "texture" => c.chart_3.0,
        "audio" => c.chart_4.0,
        "scene" => c.chart_5.0,
        "prefab" => c.chart_6.0,
        "script" => c.info.0,
        "vfx" => c.warning.0,
        _ => c.success.0,
    }
}

/// Opens `path` in Assets, remembering where it was for Back.
fn open_folder(s: &mut Studio, path: &str) {
    let now = s.editor.state.asset_folder.clone();
    if now != path {
        s.local.assets_view.back.push(now);
        s.local.assets_view.forward.clear();
    }
    s.act("editor.workspace", json!({"folder": path}));
}

/// Back (or forward, `ahead`) through the folders Assets showed.
fn step_folder(s: &mut Studio, ahead: bool) {
    let view = &mut s.local.assets_view;
    let (from, to) = if ahead {
        (&mut view.forward, &mut view.back)
    } else {
        (&mut view.back, &mut view.forward)
    };
    if let Some(path) = from.pop() {
        to.push(s.editor.state.asset_folder.clone());
        s.act("editor.workspace", json!({"folder": path}));
    }
}

/// The folder that holds `path` ("" at the project root).
fn parent(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit_once('/')
        .map_or("", |(p, _)| p)
}

/// The icon for an asset kind (its extension or document kind).
fn kind_icon(a: &Asset) -> stillkit::icons::Icon {
    match a.document_kind.as_deref().unwrap_or(a.kind.as_str()) {
        "folder" => icons::FOLDER,
        "scene" => icons::CLAPPERBOARD,
        "prefab" => icons::BOXES,
        "material" | "graph" => icons::PALETTE,
        "gltf" | "glb" | "fbx" | "obj" => icons::BOX,
        "png" | "jpg" | "jpeg" | "exr" | "hdr" | "ktx2" => icons::IMAGE,
        "wav" | "ogg" | "mp3" | "flac" | "music" => icons::AUDIO_LINES,
        "cs" => icons::FILE_CODE,
        "animation" => icons::PERSON_STANDING,
        "terrain" => icons::MOUNTAIN,
        "vfx" => icons::SPARKLES,
        _ => icons::FILE,
    }
}

/// Assets: breadcrumb and search over a grid of tiles; a folder opens on double-click, a
/// scene opens in the editor.
fn assets(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let folder = s.editor.state.asset_folder.clone();
    let query = s
        .editor
        .state
        .searches
        .get("assets")
        .cloned()
        .unwrap_or_default();
    let mut crumbs = row().gap(2.0).align(Align::Center).min_w(0.0).child(crumb(
        l,
        "Project",
        String::new(),
        folder.is_empty(),
    ));
    let mut at = String::new();
    for part in folder.split('/').filter(|p| !p.is_empty()) {
        if !at.is_empty() {
            at.push('/');
        }
        at.push_str(part);
        crumbs = crumbs
            .child(
                icon(icons::CHEVRON_RIGHT)
                    .size(12.0)
                    .color(c.muted_foreground.0),
            )
            .child(crumb(l, part, at.clone(), at == folder));
    }
    let search = search(
        l,
        "assets/search",
        "Search assets",
        &query,
        None,
        |f| f.on_input(|s: &mut Studio, t| s.act("editor.workspace", json!({"assetQuery": t}))),
        |s: &mut Studio| {
            // Kept for Restore Removed Assets Search.
            let query = s
                .editor
                .state
                .searches
                .get("assets")
                .cloned()
                .unwrap_or_default();
            if !query.trim().is_empty() {
                s.local.assets_cleared_query = Some(query);
            }
            s.act("editor.workspace", json!({"assetQuery": ""}))
        },
    )
    .h(22.0)
    .w(200.0);
    let view = &s.local.assets_view;
    let nav = |glyph, label: &str, key: &str, on: bool, ahead: bool| {
        enabled(
            icon_button(l, Tone::Ghost, Size::Sm, glyph, label)
                .size(22.0)
                .key(key.to_owned())
                .on_press(move |s: &mut Studio| step_folder(s, ahead)),
            on,
        )
    };
    let mut add = button(l, Tone::Ghost, Size::Sm, Some(icons::PLUS), "Add")
        .key("assets/add")
        .h(22.0)
        .on_press(|s: &mut Studio| s.local.assets_view.add = !s.local.assets_view.add);
    if view.add {
        let items = super::asset_create::KINDS
            .iter()
            .map(|(kind, title, glyph)| {
                let kind = *kind;
                menu_item(
                    l,
                    &format!("assets/add/{kind}"),
                    Some(*glyph),
                    title,
                    None,
                    false,
                )
                .on_press(move |s: &mut Studio| {
                    s.local.assets_view.add = false;
                    super::asset_create::create(s, kind);
                })
            })
            .collect();
        add = add.overlay(
            stillkit::el::menu(menu_panel(l, items))
                .side(stillkit::el::Side::Bottom)
                .align(Align::Start)
                .on_dismiss(|s: &mut Studio| s.local.assets_view.add = false),
        );
    }
    // List, then three tile sizes (Unity's slider, as steps).
    // One Tab stop for the view sizes (on the current one), arrows between them.
    let mut sizes = row()
        .gap(1.0)
        .focus_group(stillkit::el::Orientation::Horizontal);
    for (size, glyph, label) in [
        (TileSize::List, icons::LIST, "List"),
        (TileSize::Small, icons::GRID_3X3, "Small tiles"),
        (TileSize::Medium, icons::GRID_2X2, "Tiles"),
        (TileSize::Large, icons::LAYOUT_GRID, "Large tiles"),
    ] {
        sizes = sizes.child(
            toggle_button(l, Size::Sm, glyph, label, view.size == size)
                .size(22.0)
                .key(format!("assets/size/{label}"))
                .on_press(move |s: &mut Studio| s.local.assets_view.size = size),
        );
    }
    let toolbar = row()
        .h(super::style::TOOLBAR)
        .pad_x(4.0)
        .gap(4.0)
        .align(Align::Center)
        .child(nav(
            icons::ARROW_LEFT,
            "Back",
            "assets/back",
            !view.back.is_empty(),
            false,
        ))
        .child(nav(
            icons::ARROW_RIGHT,
            "Forward",
            "assets/forward",
            !view.forward.is_empty(),
            true,
        ))
        .child(add)
        .child(crumbs.grow(1.0).pad_x(4.0))
        .child(search)
        .children((!query.trim().is_empty()).then(|| {
            icon_button(
                l,
                Tone::Ghost,
                Size::Sm,
                icons::BOOKMARK,
                "Save this search",
            )
            .size(22.0)
            .key("assets/search/save")
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "assets.search.save", &crate::catalog::Target::None)
            })
        }))
        .child(sizes)
        .child(view_menu(s));
    // Type filters (Unreal's filter bar): one type across the whole project. A narrow
    // panel wraps them onto another line rather than hiding the last ones past its edge.
    let mut chips = row()
        .min_w(0.0)
        .pad_x(6.0)
        .pad_y(2.0)
        .gap(3.0)
        .align(Align::Center)
        .flow_wrap()
        .focus_group(stillkit::el::Orientation::Horizontal);
    for (id, label, glyph) in FILTERS {
        let on = view.filter == Some(id);
        let tint = category_tint(l, id);
        let hover = c.hover_surface.0;
        chips = chips.child(
            super::style::chosen(l, row(), on)
                .key(format!("assets/filter/{id}"))
                .h(20.0)
                .pad_x(7.0)
                .gap(5.0)
                .align(Align::Center)
                .radius(10.0)
                .label(label)
                .selected(on)
                .hovered(move |e| if on { e } else { e.fill(hover) })
                .on_press(move |s: &mut Studio| {
                    let f = &mut s.local.assets_view.filter;
                    *f = if *f == Some(id) { None } else { Some(id) };
                })
                .child(el().size(6.0).radius(3.0).fill(tint))
                .child(icon(glyph).size(12.0).color(if on {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                }))
                .child(text(label).text_size(11.0).color(if on {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                })),
        );
    }
    let toolbar = col().child(toolbar).child(chips);
    let Some(browser) = s.assets.as_ref() else {
        return col().grow(1.0).child(toolbar).child(
            row()
                .grow(1.0)
                .justify(Justify::Center)
                .align(Align::Center)
                .child(spinner(l, 18.0)),
        );
    };
    let needle = query.trim().to_lowercase();
    let mut items: Vec<Asset> = browser
        .items
        .iter()
        .filter(|a| {
            let hidden = a
                .path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .is_some_and(|n| n.starts_with('.'));
            // Support files, and the project's own files at its root (ara.project.json,
            // notes), show with Show All Files.
            let root_file = a.kind != "folder" && !a.path.trim_end_matches('/').contains('/');
            if hidden || (!s.local.assets_view.show_all && (a.is_noise() || root_file)) {
                false
            } else if let Some(only) = s.local.assets_view.filter {
                category(a) == Some(only)
                    && (needle.is_empty() || a.path.to_lowercase().contains(&needle))
            } else if needle.is_empty() {
                parent(&a.path) == folder
            } else {
                a.kind != "folder" && a.path.to_lowercase().contains(&needle)
            }
        })
        .cloned()
        .collect();
    // Folders the scan lists only through their files show too.
    if needle.is_empty() && s.local.assets_view.filter.is_none() {
        let prefix = if folder.is_empty() {
            String::new()
        } else {
            format!("{folder}/")
        };
        let mut dirs = std::collections::BTreeSet::new();
        for a in &browser.items {
            if let Some((dir, _)) = a
                .path
                .strip_prefix(&prefix)
                .and_then(|rest| rest.split_once('/'))
                && !dir.is_empty()
                && !dir.starts_with('.')
            {
                dirs.insert(format!("{prefix}{dir}"));
            }
        }
        for dir in dirs {
            if !items.iter().any(|a| a.path.trim_end_matches('/') == dir) {
                items.push(Asset::folder(dir, None));
            }
        }
    }
    items.sort_by(|a, b| {
        (b.kind == "folder")
            .cmp(&(a.kind == "folder"))
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    let footer = assets_footer(s, &folder, items.len());
    let body = if let Some(error) = &browser.error {
        empty(
            l,
            icons::CIRCLE_ALERT,
            "Assets could not be read",
            error,
            None,
        )
    } else if items.is_empty() {
        if needle.is_empty() {
            empty(
                l,
                icons::FOLDER_OPEN,
                "This folder is empty",
                "Drop files here from Finder or Explorer to import them, or use Import… in ⋯.",
                None,
            )
        } else {
            empty(
                l,
                icons::SEARCH,
                "No assets match",
                "Search looks at file paths across the project.",
                None,
            )
        }
    } else {
        let selected = s.editor.state.asset_path.clone();
        let counts = contents(browser);
        let size = s.local.assets_view.size;
        let tiles: Vec<El<Studio>> = items
            .into_iter()
            .map(|a| {
                let preview = s.previews.images.get(&a.path).cloned();
                let held = (a.kind == "folder").then(|| {
                    counts
                        .get(a.path.trim_end_matches('/'))
                        .copied()
                        .unwrap_or(0)
                });
                let renaming = s
                    .local
                    .asset_rename
                    .as_ref()
                    .filter(|(path, _)| *path == a.path)
                    .map(|(_, name)| name.as_str());
                tile(
                    l,
                    &a,
                    selected.as_deref() == Some(a.path.as_str()),
                    preview,
                    held,
                    size,
                    renaming,
                )
            })
            .collect();
        let here = folder.clone();
        col()
            .grow(1.0)
            .min_h(0.0)
            .scroll(Axis::Vertical)
            // Empty space: the folder's own menu (create, refresh, import), as in Unity.
            .on_context_menu(move |s: &mut Studio, x, y| {
                use crate::catalog::{Target, menus::Context};
                s.local.context = Some((Context::Assets, Target::Folder(here.clone()), (x, y)));
            })
            .child(if size == TileSize::List {
                col().pad(4.0).gap(1.0).children(tiles)
            } else {
                row().flow_wrap().pad(6.0).gap(2.0).children(tiles)
            })
    };
    // Unity's Project window: the folder tree beside the folder's contents.
    let right = col()
        .w(0.0)
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar)
        .children(import_strip(s))
        .child(body)
        .child(footer);
    // The folder tree beside the grid, unless hidden (one column, as Unity can).
    let tree = s
        .editor
        .state
        .settings
        .get("assets.folderTree")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut root = row()
        .grow(1.0)
        .min_h(0.0)
        .children(tree.then(|| folder_tree(s, browser, &folder)))
        .child(right);
    if let Some(dialog) = conflicts_dialog(s).or_else(|| move_dialog(s, browser)) {
        root = root.overlay(dialog);
    }
    if let Some(path) = s.local.asset_delete.clone() {
        let name = path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .to_owned();
        let confirm = path.clone();
        let panel = dialog_panel(
            l,
            Some((Status::Danger, icons::TRASH)),
            &format!("Delete {name}?"),
            text("The file goes from the project. Files other assets still use are kept.")
                .text_size(12.0)
                .color(c.muted_foreground.0)
                .wrap(3),
            vec![
                button(l, Tone::Secondary, Size::Md, None, "Cancel")
                    .key("asset/delete/cancel")
                    .on_press(|s: &mut Studio| s.local.asset_delete = None),
                button(l, Tone::Danger, Size::Md, Some(icons::TRASH), "Delete")
                    .key("asset/delete/confirm")
                    .on_press(move |s: &mut Studio| super::asset_ops::delete(s, &confirm)),
            ],
            |s: &mut Studio| s.local.asset_delete = None,
        )
        .w(380.0);
        root = root.overlay(
            stillkit::el::dialog(panel).on_dismiss(|s: &mut Studio| s.local.asset_delete = None),
        );
    }
    root
}

/// Under the contents, as Unity's Project window: the selected asset's path, or what the
/// folder shows (`shown` items) and where it is.
fn assets_footer(s: &Studio, folder: &str, shown: usize) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let (glyph, line) = match s.editor.state.asset_path.as_deref() {
        Some(path) if !path.is_empty() => (icons::FILE, path.to_owned()),
        _ => {
            let place = if folder.is_empty() { "Project" } else { folder };
            let items = match shown {
                1 => "1 item".to_owned(),
                n => format!("{n} items"),
            };
            (icons::FOLDER, format!("{place} · {items}"))
        }
    };
    row()
        .key("assets/footer")
        .h(22.0)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .fill(super::style::tones(l).band)
        .label(line.clone())
        .child(icon(glyph).size(12.0).color(c.muted_foreground.0))
        .child(
            text(line)
                .text_size(11.0)
                .family(mono())
                .wrap(1)
                .color(c.muted_foreground.0)
                .grow(1.0)
                .min_w(0.0),
        )
}

/// The Assets view menu (⋯): Import, what shows (every file, the folder only), thumbnails.
fn view_menu(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let open = s.local.assets_view.menu;
    let mut b = icon_button(l, Tone::Ghost, Size::Sm, icons::ELLIPSIS, "More")
        .size(22.0)
        .key("assets/menu")
        .expanded(open)
        .on_press(|s: &mut Studio| s.local.assets_view.menu = !s.local.assets_view.menu);
    if !open {
        return b;
    }
    let run = |id: &'static str| {
        move |s: &mut Studio| {
            s.local.assets_view.menu = false;
            super::commands::run(s, id, &crate::catalog::Target::None);
        }
    };
    let tree = s
        .editor
        .state
        .settings
        .get("assets.folderTree")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let items = vec![
        menu_item(
            l,
            "assets/menu/import",
            Some(icons::DOWNLOAD),
            "Import…",
            None,
            false,
        )
        .on_press(run("assets.import")),
        menu_separator(l),
        menu_check(
            l,
            "assets/menu/all",
            "Show All Files",
            s.local.assets_view.show_all,
        )
        .on_press(run("assets.show_all")),
        menu_check(
            l,
            "assets/menu/scope",
            "Search Within Current Folder",
            s.editor.state.asset_search_folder_only,
        )
        .on_press(run("assets.search.scope")),
        menu_check(l, "assets/menu/tree", "Folder Tree", tree).on_press(move |s: &mut Studio| {
            s.local.assets_view.menu = false;
            s.act(
                "editor.settings.set",
                json!({"key": "assets.folderTree", "value": !tree}),
            );
        }),
        menu_separator(l),
        menu_item(
            l,
            "assets/menu/thumbnails",
            Some(icons::IMAGE),
            "Rebuild Thumbnails",
            None,
            false,
        )
        .on_press(run("assets.rebuild_thumbnails")),
        menu_item(
            l,
            "assets/menu/refresh",
            Some(icons::REFRESH_CW),
            "Refresh",
            None,
            false,
        )
        .on_press(run("assets.refresh")),
    ];
    b = b.overlay(
        stillkit::el::menu(menu_panel(l, items))
            .side(stillkit::el::Side::Bottom)
            .align(Align::End)
            .on_dismiss(|s: &mut Studio| s.local.assets_view.menu = false),
    );
    b
}

/// Every folder Assets knows (listed, or holding a listed file), without a trailing `/`.
fn folders(browser: &crate::assets::Browser) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for a in &browser.items {
        let path = a.path.trim_end_matches('/');
        let mut at = if a.kind == "folder" {
            path
        } else {
            parent(path)
        };
        while !at.is_empty() && out.insert(at.to_owned()) {
            at = parent(at);
        }
    }
    out
}

/// The keys of every folder an asset can be dropped on (tree rows, folder tiles), for the
/// host to report where they lie while an asset is dragged.
pub(super) fn drop_keys(s: &Studio) -> Vec<String> {
    let Some(browser) = s.assets.as_ref() else {
        return Vec::new();
    };
    let mut keys = vec!["assets/tree/".to_owned(), "hierarchy/rows".to_owned()];
    // The Hierarchy's rows (those on screen report a rect) and the Inspector's asset fields.
    keys.extend(s.resolved.keys().map(|id| format!("row/{id}")));
    keys.extend(
        super::inspector::asset_fields(s)
            .into_iter()
            .map(|p| format!("field/{p}")),
    );
    for folder in folders(browser) {
        keys.push(format!("asset/{folder}"));
        keys.push(format!("asset/{folder}/"));
        keys.push(format!("assets/tree/{folder}"));
    }
    keys
}

/// The folder under window point `at` (a tree row or a folder tile), `""` for Project.
fn folder_at(s: &Studio, at: (f32, f32)) -> Option<String> {
    s.local.rects.iter().find_map(|(key, r)| {
        let folder = key
            .strip_prefix("assets/tree/")
            .or_else(|| key.strip_prefix("asset/"))?;
        (at.0 >= r.left && at.0 <= r.right && at.1 >= r.top && at.1 <= r.bottom)
            .then(|| folder.trim_end_matches('/').to_owned())
    })
}

/// Favourites and saved searches, above the folder tree (Unity's Favorites).
fn bookmarks(s: &Studio) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let state = &s.editor.state;
    let mut out = Vec::new();
    let heading = |label: &str| {
        row().h(22.0).pad_x(6.0).align(Align::Center).child(
            text(label.to_owned())
                .text_size(11.0)
                .weight(600)
                .color(c.muted_foreground.0),
        )
    };
    let line = |key: String, glyph, name: String, on_press: Box<dyn Fn(&mut Studio)>| {
        let hover = c.hover_surface.0;
        row()
            .key(key)
            .h(super::style::ROW)
            .pad_x(6.0)
            .gap(6.0)
            .align(Align::Center)
            .radius(super::style::RADIUS)
            .label(name.clone())
            .hovered(move |e| e.fill(hover))
            .on_press(move |s: &mut Studio| on_press(s))
            .child(icon(glyph).size(12.0).color(c.warning.0))
            .child(
                text(name)
                    .text_size(12.0)
                    .color(c.foreground.0)
                    .wrap(1)
                    .w(0.0)
                    .grow(1.0),
            )
    };
    if !state.asset_favourites.is_empty() {
        out.push(heading("Favourites"));
        for path in state.asset_favourites.keys() {
            let name = path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(path);
            let target = crate::catalog::Target::Text(path.clone());
            out.push(line(
                format!("assets/favourite/{path}"),
                icons::STAR,
                if name.is_empty() { "Project" } else { name }.to_owned(),
                Box::new(move |s| super::commands::run(s, "assets.favourite.open", &target)),
            ));
        }
    }
    if !state.asset_saved_searches.is_empty() {
        out.push(heading("Searches"));
        for name in state.asset_saved_searches.keys() {
            let (open, remove) = (
                crate::catalog::Target::Text(name.clone()),
                crate::catalog::Target::Text(name.clone()),
            );
            out.push(
                line(
                    format!("assets/saved/{name}"),
                    icons::SEARCH,
                    name.clone(),
                    Box::new(move |s| super::commands::run(s, "assets.search.apply", &open)),
                )
                .child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Remove this search")
                        .size(18.0)
                        .key(format!("assets/saved/{name}/remove"))
                        .on_press(move |s: &mut Studio| {
                            super::commands::run(s, "assets.search.remove", &remove)
                        }),
                ),
            );
        }
    }
    if !out.is_empty() {
        out.push(el().h(6.0));
    }
    out
}

/// The file a stack frame names, when it may be opened: a relative path (no root, drive,
/// UNC prefix or `..`, in either separator) that, canonicalised, lies inside the project
/// `root`. Frames come from scripts, so anything else is shown as plain text and never
/// handed to an opener.
pub(super) fn frame_target(root: &std::path::Path, file: &str) -> Option<std::path::PathBuf> {
    use std::path::{Component, Path};
    let portable = file.replace('\\', "/");
    let relative = Path::new(&portable);
    let plain = !portable.starts_with('/')
        && !portable.contains(':')
        && relative
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if !plain {
        return None;
    }
    let root = std::fs::canonicalize(root).ok()?;
    let target = std::fs::canonicalize(root.join(relative)).ok()?;
    target.starts_with(&root).then_some(target)
}

/// Opens `file` at `line`: a script in the IDE, anything else where the system opens it;
/// only inside the project (`frame_target`).
fn open_source(s: &mut Studio, file: &str, line: Option<u32>, column: Option<u32>) {
    let Some(target) = frame_target(s.editor.host.root(), file) else {
        s.editor.report_failure(&*anyhow::anyhow!(
            "{file} is not a file in this project, so it was not opened"
        ));
        return;
    };
    let path = target.to_string_lossy().into_owned();
    if file.ends_with(".cs") {
        let at = line.map(|l| (l, column.unwrap_or(1)));
        super::asset_ops::open_in_ide(s, &path, at);
    } else if let Err(error) = crate::system::open_external(&target) {
        s.editor.report_failure(&*anyhow::anyhow!("{error}"));
    }
}

/// Whether a diagnostic carries an exception stack (a C# fault).
fn has_stack(d: &ara_diag::Diagnostic) -> bool {
    d.data
        .as_deref()
        .is_some_and(|data| data.get("exception").is_some())
}

/// A C# fault's stack, as Unity's Console shows it: each exception (inner ones as "Caused
/// by"), its type and message, then its frames, a frame's file and line a link that opens
/// it. Empty for anything else.
fn stack(s: &Studio, d: &ara_diag::Diagnostic) -> Vec<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let Some(value) = d.data.as_deref().and_then(|data| data.get("exception")) else {
        return Vec::new();
    };
    let stack = crate::exception_stack::Stack::parse(value);
    let mut out = Vec::new();
    for (i, exception) in stack.nodes.iter().enumerate() {
        let title = match i {
            0 => format!("{}: {}", exception.kind, exception.message),
            _ => format!("Caused by {}: {}", exception.kind, exception.message),
        };
        let indent = exception.depth as f32 * 10.0;
        out.push(
            row().child(el().w(indent)).child(
                text(title)
                    .text_size(11.5)
                    .weight(600)
                    .wrap(3)
                    .color(c.foreground.0)
                    .w(0.0)
                    .grow(1.0),
            ),
        );
        for frame in &exception.frames {
            let mut line = row()
                .gap(8.0)
                .align(Align::Center)
                .child(el().w(indent + 10.0))
                .child(
                    text(format!("at {}", frame.method))
                        .text_size(11.0)
                        .family(mono())
                        .color(c.muted_foreground.0)
                        .wrap(1)
                        .w(0.0)
                        .grow(1.0),
                );
            let root = s.editor.host.root();
            if let Some(file) = frame
                .file
                .filter(|f| !frame.link || frame_target(root, f).is_none())
            {
                // A file outside the project (or not a source) reads, never opens.
                let shown = match frame.line {
                    Some(n) => format!("{file}:{n}"),
                    None => file.to_owned(),
                };
                line = line.child(
                    text(shown)
                        .key(format!("console/detail/source/{}", frame.index))
                        .text_size(11.0)
                        .family(mono())
                        .color(c.muted_foreground.0),
                );
            } else if let (true, Some(file)) = (frame.link, frame.file) {
                let shown = match frame.line {
                    Some(n) => format!("{}:{n}", file.rsplit(['/', '\\']).next().unwrap_or(file)),
                    None => file.to_owned(),
                };
                let (file, at, column) = (file.to_owned(), frame.line, frame.column);
                line = line.child(
                    button(l, Tone::Ghost, Size::Sm, None, &shown)
                        .key(format!("console/detail/frame/{}", frame.index))
                        .tooltip(format!("Open {file}"))
                        .on_press(move |s: &mut Studio| open_source(s, &file, at, column)),
                );
            }
            out.push(line);
        }
    }
    if stack.truncated {
        out.push(
            text("The stack is longer; the rest was cut.")
                .text_size(11.0)
                .color(c.muted_foreground.0),
        );
    }
    out
}

/// A running import (its message, progress and Cancel), or how the last one ended.
fn import_strip(s: &Studio) -> Option<El<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let strip = || {
        row()
            .key("assets/import")
            .h(26.0)
            .pad_x(8.0)
            .gap(8.0)
            .align(Align::Center)
            .fill(super::style::tones(l).band)
    };
    if let Some((message, fraction)) = s.importer.progress() {
        let mut out = strip()
            .child(spinner(l, 12.0))
            .child(
                text(message.to_owned())
                    .text_size(11.5)
                    .color(c.foreground.0)
                    .wrap(1),
            )
            .child(progress(l, fraction, Status::Info).w(120.0))
            .child(el().grow(1.0));
        if s.importer.can_cancel() {
            out = out.child(
                button(l, Tone::Ghost, Size::Sm, None, "Cancel")
                    .key("assets/import/cancel")
                    .on_press(|s: &mut Studio| {
                        super::commands::run(
                            s,
                            "assets.import.cancel",
                            &crate::catalog::Target::None,
                        )
                    }),
            );
        }
        return Some(out);
    }
    let note = s.importer.note.clone()?;
    Some(
        strip()
            .child(icon(icons::DOWNLOAD).size(12.0).color(c.muted_foreground.0))
            .child(
                text(note)
                    .text_size(11.5)
                    .color(c.muted_foreground.0)
                    .wrap(1)
                    .w(0.0)
                    .grow(1.0),
            )
            .child(
                icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Dismiss")
                    .size(18.0)
                    .key("assets/import/dismiss")
                    .on_press(|s: &mut Studio| s.importer.note = None),
            ),
    )
}

/// Files the import would overwrite: Rename keeps both, Replace updates the assets, Skip
/// leaves them (Unity asks the same).
fn conflicts_dialog(s: &Studio) -> Option<stillkit::el::Overlay<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let conflicts = s.importer.conflicts.as_ref()?;
    let count = conflicts.labels.len();
    let mut list = col().gap(2.0);
    for label in conflicts.labels.iter().take(8) {
        list = list.child(
            text(label.clone())
                .text_size(11.5)
                .family(mono())
                .color(c.foreground.0)
                .wrap(1),
        );
    }
    if count > 8 {
        list = list.child(
            text(format!("… and {} more", count - 8))
                .text_size(11.5)
                .color(c.muted_foreground.0),
        );
    }
    let choose = |policy: Option<&'static str>| {
        move |s: &mut Studio| {
            if let Err(error) = s.importer.resolve(&mut s.editor, policy) {
                s.editor.report_failure(&*anyhow::anyhow!("{error}"));
            }
        }
    };
    let panel = dialog_panel(
        l,
        Some((Status::Warning, icons::TRIANGLE_ALERT)),
        &match count {
            1 => "1 file already exists".to_owned(),
            n => format!("{n} files already exist"),
        },
        col()
            .gap(8.0)
            .child(
                text("Rename keeps both copies. Replace updates the existing assets in place. Skip leaves them as they are.")
                    .text_size(12.0)
                    .wrap(3)
                    .color(c.muted_foreground.0),
            )
            .child(list),
        vec![
            button(l, Tone::Ghost, Size::Md, None, "Cancel")
                .key("assets/conflict/cancel")
                .on_press(choose(None)),
            button(l, Tone::Secondary, Size::Md, None, "Skip")
                .key("assets/conflict/skip")
                .on_press(choose(Some("skip"))),
            button(l, Tone::Secondary, Size::Md, None, "Replace")
                .key("assets/conflict/replace")
                .on_press(choose(Some("replace"))),
            button(l, Tone::Primary, Size::Md, None, "Rename")
                .key("assets/conflict/rename")
                .on_press(choose(Some("rename"))),
        ],
        choose(None),
    )
    .w(460.0);
    Some(stillkit::el::dialog(panel).on_dismiss(choose(None)))
}

/// Move to Folder: the project's folders to pick from, searched as you type.
fn move_dialog(
    s: &Studio,
    browser: &crate::assets::Browser,
) -> Option<stillkit::el::Overlay<Studio>> {
    let l = &s.look;
    let c = &l.c;
    let (path, query) = s.local.assets_view.moving.as_ref()?;
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    let own = path.trim_end_matches('/');
    let needle = query.trim().to_lowercase();
    let mut list = col().gap(1.0).max_h(320.0).scroll(Axis::Vertical);
    let choices = std::iter::once(String::new())
        .chain(folders(browser))
        // Not into itself or its own folder, nor anywhere inside itself.
        .filter(|f| *f != own && *f != parent(own) && !f.starts_with(&format!("{own}/")))
        .filter(|f| needle.is_empty() || f.to_lowercase().contains(&needle));
    let hover = c.hover_surface.0;
    for folder in choices.take(200) {
        let (source, to) = (path.clone(), folder.clone());
        list = list.child(
            row()
                .key(format!("assets/move/{folder}"))
                .h(super::style::ROW)
                .pad_x(8.0)
                .gap(6.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(if folder.is_empty() {
                    "Project".to_owned()
                } else {
                    folder.clone()
                })
                .hovered(move |e| e.fill(hover))
                .on_press(move |s: &mut Studio| super::asset_ops::move_to(s, &source, &to))
                .child(icon(icons::FOLDER).size(13.0).color(folder_tint(l, false)))
                .child(
                    text(if folder.is_empty() {
                        "Project".to_owned()
                    } else {
                        folder
                    })
                    .text_size(12.0)
                    .color(c.foreground.0)
                    .wrap(1),
                ),
        );
    }
    let panel = dialog_panel(
        l,
        Some((Status::Info, icons::FOLDER_OPEN)),
        &format!("Move {name} to…"),
        col()
            .gap(8.0)
            .child(search(
                l,
                "assets/move/search",
                "Search folders",
                query,
                None,
                |f| {
                    f.on_input(|s: &mut Studio, t| {
                        if let Some((_, q)) = s.local.assets_view.moving.as_mut() {
                            *q = t.to_owned();
                        }
                    })
                },
                |s: &mut Studio| {
                    if let Some((_, q)) = s.local.assets_view.moving.as_mut() {
                        q.clear();
                    }
                },
            ))
            .child(list)
            .child(
                text("References to it update in the same step.")
                    .text_size(11.5)
                    .color(c.muted_foreground.0),
            ),
        vec![
            button(l, Tone::Ghost, Size::Md, None, "Cancel")
                .key("assets/move/cancel")
                .on_press(|s: &mut Studio| s.local.assets_view.moving = None),
        ],
        |s: &mut Studio| s.local.assets_view.moving = None,
    )
    .w(420.0);
    Some(stillkit::el::dialog(panel).on_dismiss(|s: &mut Studio| s.local.assets_view.moving = None))
}

/// How many visible entries (files and subfolders) each folder holds directly.
fn contents(browser: &crate::assets::Browser) -> std::collections::HashMap<String, usize> {
    let mut seen = std::collections::HashSet::new();
    let mut counts = std::collections::HashMap::new();
    for a in &browser.items {
        let mut path = a.path.trim_end_matches('/');
        // The entry, and every folder above it, counts once in its parent.
        while !path.is_empty() {
            if path.rsplit('/').next().is_some_and(|n| n.starts_with('.')) {
                break;
            }
            if seen.insert(path.to_owned()) {
                *counts.entry(parent(path).to_owned()).or_insert(0) += 1;
            }
            path = parent(path);
        }
    }
    counts
}

/// A folder's icon: the plain outline when empty, with contents lines when not.
fn folder_glyph(empty: bool) -> stillkit::icons::Icon {
    if empty {
        icons::FOLDER
    } else {
        icons::FOLDER_FULL
    }
}

/// Folders' one colour, a little dimmer when empty.
fn folder_tint(l: &ara_kit::look::Look, empty: bool) -> stillkit::Rgba {
    l.c.warning.0.with_alpha(if empty { 0.6 } else { 1.0 })
}

/// Every folder in the project, as a tree: the open ones and the current one's ancestors
/// show their children. A click opens the folder in the grid.
fn folder_tree(s: &Studio, browser: &crate::assets::Browser, current: &str) -> El<Studio> {
    let l = &s.look;
    let counts = contents(browser);
    let mut dirs = std::collections::BTreeSet::new();
    for a in &browser.items {
        let path = a.path.trim_end_matches('/');
        let mut at = if a.kind == "folder" {
            path
        } else {
            parent(path)
        };
        while !at.is_empty() {
            if !at.rsplit('/').next().is_some_and(|n| n.starts_with('.')) {
                dirs.insert(at.to_owned());
            }
            at = parent(at);
        }
    }
    let open = |d: &str| {
        s.editor.state.asset_expanded_folders.contains(d)
            || current == d
            || current.starts_with(&format!("{d}/"))
    };
    let mut tree = col()
        .w(200.0)
        .min_w(160.0)
        .pad(4.0)
        .gap(0.0)
        .scroll(Axis::Vertical);
    tree = tree.children(bookmarks(s)).child(folder_row(
        l,
        "Project",
        String::new(),
        0,
        current.is_empty(),
        None,
        false,
    ));
    for dir in &dirs {
        // Shown when every ancestor is open.
        let mut at = parent(dir);
        let mut visible = true;
        while !at.is_empty() {
            visible &= open(at);
            at = parent(at);
        }
        if !visible {
            continue;
        }
        let depth = dir.matches('/').count() + 1;
        let has_children = dirs.iter().any(|d| parent(d) == dir);
        let expanded = has_children.then(|| open(dir));
        let name = dir.rsplit('/').next().unwrap_or(dir);
        let empty = counts.get(dir.as_str()).copied().unwrap_or(0) == 0;
        tree = tree.child(folder_row(
            l,
            name,
            dir.clone(),
            depth,
            current == dir,
            expanded,
            empty,
        ));
    }
    tree
}

/// A folder tree row: disclosure (when it has subfolders), icon and name.
fn folder_row(
    l: &ara_kit::look::Look,
    name: &str,
    path: String,
    depth: usize,
    current: bool,
    expanded: Option<bool>,
    empty: bool,
) -> El<Studio> {
    let c = &l.c;
    let hover = c.hover_surface.0;
    let (fill, ink) = if current {
        (c.selection_surface.0, c.selection_foreground.0)
    } else {
        (stillkit::Rgba::TRANSPARENT, c.foreground.0)
    };
    // The shape tells what a folder holds; its colour stays one colour (later a user
    // choice), only dimmed when empty.
    let glyph = if path.is_empty() {
        icons::BOX
    } else {
        folder_glyph(empty)
    };
    let folder_ink = if current { ink } else { folder_tint(l, empty) };
    let disclosure = match expanded {
        Some(on) => {
            let path = path.clone();
            el().size(16.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .color(c.muted_foreground.0)
                .cascade_ink()
                .label(if on { "Collapse" } else { "Expand" })
                .on_press(move |s: &mut Studio| {
                    let set = &mut s.editor.state.asset_expanded_folders;
                    if !set.remove(&path) {
                        set.insert(path.clone());
                    }
                })
                .child(
                    icon(if on {
                        icons::CHEVRON_DOWN
                    } else {
                        icons::CHEVRON_RIGHT
                    })
                    .size(11.0),
                )
        }
        None => el().size(16.0),
    };
    let open = path.clone();
    super::style::chosen(l, row(), current)
        .key(format!("assets/tree/{path}"))
        .h(super::style::ROW)
        .pad_x(4.0)
        .gap(4.0)
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(fill)
        .color(ink)
        .cascade_ink()
        .label(name.to_owned())
        .selected(current)
        .hovered(move |e| if current { e } else { e.fill(hover) })
        .on_press(move |s: &mut Studio| open_folder(s, &open))
        .child(el().w(depth as f32 * 12.0))
        .child(disclosure)
        .child(icon(glyph).size(13.0).color(folder_ink))
        .child(text(name.to_owned()).text_size(12.0).grow(1.0))
}

fn crumb(l: &ara_kit::look::Look, label: &str, path: String, current: bool) -> El<Studio> {
    let c = &l.c;
    let hover = c.foreground.0;
    row()
        .h(24.0)
        .pad_x(6.0)
        .align(Align::Center)
        .radius(l.radius.xs)
        .color(if current {
            c.foreground.0
        } else {
            c.muted_foreground.0
        })
        .cascade_ink()
        .label(label)
        .hovered(move |e| e.color(hover))
        .on_press(move |s: &mut Studio| open_folder(s, &path))
        .child(
            text(label)
                .text_size(12.0)
                .weight(if current { 600 } else { 500 }),
        )
}

/// One asset: its picture or bare icon and the name under it (two lines at most). A folder
/// with contents shows the full folder glyph, an empty one the plain outline. Only hover
/// and selection give a tile a background.
fn tile(
    l: &ara_kit::look::Look,
    a: &Asset,
    selected: bool,
    preview: Option<std::sync::Arc<stillkit::Image>>,
    held: Option<usize>,
    size: TileSize,
    renaming: Option<&str>,
) -> El<Studio> {
    let c = &l.c;
    let (width, picture) = size.tile();
    let list = size == TileSize::List;
    let kind = category(a);
    let name = a
        .path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(&a.path);
    let stem = if a.kind == "folder" {
        name
    } else {
        name.rsplit_once('.').map_or(name, |(s, _)| s)
    };
    let folder = a.kind == "folder";
    let path = a.path.clone();
    let scene = a.document_kind.as_deref() == Some("scene");
    let placeable = a.kind == "prefab" || matches!(a.kind.as_str(), "gltf" | "glb" | "fbx" | "obj");
    let reference = a.authored_path.clone();
    let empty = held == Some(0);
    let (glyph, glyph_ink) = match held {
        Some(_) => (folder_glyph(empty), folder_tint(l, empty)),
        None => (kind_icon(a), c.muted_foreground.0),
    };
    let hover = c.hover_surface.0.with_alpha(0.7);
    let shell = if list {
        super::style::chosen(l, row(), selected)
            .h(22.0)
            .pad_x(6.0)
            .gap(8.0)
    } else {
        super::style::chosen(l, col(), selected)
            .w(width)
            .pad_x(4.0)
            .pad_y(6.0)
            .gap(3.0)
    };
    let strip = kind.filter(|_| !folder && !list).map(|k| {
        el().w(picture * 0.8)
            .h(2.0)
            .radius(1.0)
            .fill(category_tint(l, k))
    });
    let picture_el = match preview {
        // The rendered picture, or the kind's bare icon until it lands.
        Some(picture_image) if !list => stillkit::el::image(picture_image).size(picture),
        _ => {
            let side = if list { 16.0 } else { picture };
            el().size(side)
                .align(Align::Center)
                .justify(Justify::Center)
                .child(
                    icon(glyph)
                        .size(if list {
                            14.0
                        } else if folder {
                            picture * 0.85
                        } else {
                            picture * 0.7
                        })
                        .color(glyph_ink),
                )
        }
    };
    shell
        .key(format!("asset/{}", a.path))
        .align(Align::Center)
        .radius(super::style::RADIUS)
        .fill(if selected {
            c.selection_surface.0
        } else {
            stillkit::Rgba::TRANSPARENT
        })
        .label(name.to_owned())
        .tooltip(a.path.clone())
        .selected(selected)
        .hovered(move |e| if selected { e } else { e.fill(hover) })
        .on_context_menu({
            let path = a.path.clone();
            move |s: &mut Studio, x, y| {
                use crate::catalog::{Target, menus::Context};
                s.act("editor.workspace", json!({"asset": path}));
                s.local.context = Some(if folder {
                    (Context::Folder, Target::Folder(path.clone()), (x, y))
                } else {
                    (Context::Asset, Target::Asset(path.clone()), (x, y))
                });
            }
        })
        .on_pointer({
            let name = stem.to_owned();
            move |s: &mut Studio, e, _| {
                use stillkit::el::PointerPhase;
                if e.button != stillkit::Button::Left {
                    return false;
                }
                match e.phase {
                    PointerPhase::Down if e.clicks == 2 && folder => open_folder(s, &path),
                    PointerPhase::Down if e.clicks == 2 && scene => {
                        if let Some(doc) = &reference {
                            s.act("editor.scene.open", json!({"scene": doc}));
                            s.resolve();
                        }
                    }
                    // A press picks it; the Inspector turns to it on release, unless it was
                    // dragged (as in Unity: dragging an asset onto an entity's field keeps
                    // that entity in the Inspector).
                    PointerPhase::Down => {
                        s.act("editor.workspace", json!({"asset": path}));
                    }
                    // Anything drags onto a folder (moving it there) or an Inspector field
                    // that takes it; models and prefabs drag into the Scene or onto the
                    // Hierarchy (under the row they land on) too.
                    PointerPhase::Move if e.travel().x.abs() + e.travel().y.abs() >= 4.0 => {
                        s.local.asset_drag = Some((path.clone(), name.clone(), (e.at.x, e.at.y)));
                    }
                    PointerPhase::Up => {
                        if s.local.asset_drag.is_none() {
                            s.local.inspect_asset = !folder;
                        }
                        if let Some((path, _, at)) = s.local.asset_drag.take() {
                            let own = path.trim_end_matches('/');
                            match folder_at(s, at) {
                                Some(folder) if folder != parent(own) && folder != own => {
                                    super::asset_ops::move_to(s, &path, &folder)
                                }
                                Some(_) => {}
                                None => match (hierarchy_at(s, at), field_at(s, at)) {
                                    (Some(parent), _) if placeable => {
                                        place_under(s, &path, parent.as_deref())
                                    }
                                    (_, Some(field)) => assign(s, &path, &field),
                                    _ if placeable => place(s, &path, at),
                                    _ => {}
                                },
                            }
                        }
                    }
                    PointerPhase::Cancel => s.local.asset_drag = None,
                    _ => return false,
                }
                true
            }
        })
        .child(picture_el)
        .children(strip)
        .child(if let Some(name) = renaming {
            // Renaming in place: Enter renames (references follow), Escape keeps the name.
            let path = a.path.clone();
            stillkit::el::text_input("asset/rename")
                .value(name.to_owned())
                .select_all_on_focus()
                .text_size(11.5)
                .w(if list { 200.0 } else { width - 8.0 })
                .h(20.0)
                .fill(super::style::tones(l).well)
                .radius(super::style::RADIUS)
                .on_input(|s: &mut Studio, t| {
                    if let Some((_, name)) = s.local.asset_rename.as_mut() {
                        *name = t.to_owned();
                    }
                })
                .on_submit(move |s: &mut Studio, t| super::asset_ops::rename(s, &path, t))
                .on_key(|s: &mut Studio, key, _| {
                    if key.0 == 0x1B {
                        s.local.asset_rename = None;
                        stillkit::el::Handled::Yes
                    } else {
                        stillkit::el::Handled::No
                    }
                })
        } else if list {
            text(stem.to_owned())
                .text_size(12.0)
                .color(c.foreground.0)
                .grow(1.0)
        } else {
            text(stem.to_owned())
                .text_size(11.5)
                .wrap(2)
                .text_align(stillkit::el::TextAlign::Center)
                .color(c.foreground.0)
        })
        .children(list.then(|| {
            let label = kind
                .and_then(|k| FILTERS.iter().find(|(id, _, _)| *id == k))
                .map_or(if folder { "Folder" } else { "File" }, |(_, label, _)| {
                    label
                });
            text(label.to_owned())
                .text_size(11.0)
                .color(c.muted_foreground.0)
        }))
}

/// A severity filter (Console, Problems): its icon in `tint` and its count, a soft fill
/// while shown; the caller says what a press flips.
fn level_toggle(
    l: &ara_kit::look::Look,
    key: String,
    on: bool,
    tint: stillkit::Rgba,
    glyph: stillkit::icons::Icon,
    label: &str,
    count: usize,
) -> El<Studio> {
    let c = &l.c;
    let hover = c.hover_surface.0;
    row()
        .key(key)
        .h(24.0)
        .pad_x(8.0)
        .gap(6.0)
        .align(Align::Center)
        .radius(l.radius.sm)
        .fill(if on {
            c.raised.0
        } else {
            stillkit::Rgba::TRANSPARENT
        })
        .color(if on {
            c.foreground.0
        } else {
            c.muted_foreground.0
        })
        .cascade_ink()
        .label(format!("{label}: {count}"))
        .tooltip(format!("{} {label}", if on { "Hide" } else { "Show" }))
        .checked(on)
        .hovered(move |e| e.fill(hover))
        .child(
            icon(glyph)
                .size(13.0)
                .color(if on { tint } else { tint.with_alpha(0.45) }),
        )
        .child(text(count.to_string()).text_size(12.0).weight(560))
}

fn severity_glyph(l: &ara_kit::look::Look, severity: ara_diag::Severity) -> El<Studio> {
    let c = &l.c;
    let (glyph, ink) = match severity {
        ara_diag::Severity::Error => (icons::CIRCLE_X, c.destructive.0),
        ara_diag::Severity::Warning => (icons::TRIANGLE_ALERT, c.warning.0),
        _ => (icons::INFO, c.info.0),
    };
    icon(glyph).size(14.0).color(ink)
}

/// Console: the bus in order, newest last; toggles filter by severity.
fn console(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let counts = [
        ara_diag::Severity::Error,
        ara_diag::Severity::Warning,
        ara_diag::Severity::Info,
    ]
    .map(|sev| s.logs.iter().filter(|r| r.severity == sev).count());
    let query = s.local.console_search.trim().to_lowercase();
    let matches = |r: &&LogRow| {
        query.is_empty()
            || r.message.to_lowercase().contains(&query)
            || r.code.to_lowercase().contains(&query)
            || r.origin.to_lowercase().contains(&query)
            || r.source
                .as_deref()
                .is_some_and(|s| s.to_lowercase().contains(&query))
    };
    // This computer's messages (its GPU loader, overlay layers) are not the project's:
    // they fold into one group above the rest.
    let machine_record = |r: &&LogRow| r.diagnostic.code == ara_diag::codes::GPU_ENVIRONMENT;
    let machine: Vec<&LogRow> = s
        .logs
        .iter()
        .filter(machine_record)
        .filter(|r| s.local.console[level(r.severity)])
        .filter(matches)
        .collect();
    let filtered = s
        .logs
        .iter()
        .filter(|r| !machine_record(r))
        .filter(|r| s.local.console[level(r.severity)])
        .filter(matches);
    // Collapse: one row per repeated record, its latest, counting them all (Unity).
    let shown: Vec<LogRow> = if s.local.console_collapse {
        let mut merged: Vec<LogRow> = Vec::new();
        let mut at: std::collections::HashMap<(String, String, Option<String>), usize> =
            std::collections::HashMap::new();
        for r in filtered {
            let key = (r.code.clone(), r.message.clone(), r.source.clone());
            match at.get(&key) {
                Some(&i) => {
                    let count = merged[i].count + r.count.max(1);
                    merged[i] = LogRow { count, ..r.clone() };
                }
                None => {
                    at.insert(key, merged.len());
                    merged.push(LogRow {
                        count: r.count.max(1),
                        ..r.clone()
                    });
                }
            }
        }
        merged
    } else {
        filtered.cloned().collect()
    };
    // Severity filters: the icon in its colour and the count, a soft fill while shown.
    let c = &l.c;
    let toggle = |i: usize, glyph, label: &str| {
        let tint = [c.destructive.0, c.warning.0, c.info.0][i];
        level_toggle(
            l,
            format!("console/level/{i}"),
            s.local.console[i],
            tint,
            glyph,
            label,
            counts[i],
        )
        .on_press(move |s: &mut Studio| s.local.console[i] = !s.local.console[i])
    };
    let toolbar = row()
        .h(super::style::TOOLBAR)
        .pad_x(8.0)
        .gap(4.0)
        .align(Align::Center)
        .child(toggle(0, icons::CIRCLE_X, "Errors"))
        .child(toggle(1, icons::TRIANGLE_ALERT, "Warnings"))
        .child(toggle(2, icons::INFO, "Messages"))
        .child(
            row().w(220.0).child(
                search(
                    l,
                    "console/search",
                    "Search messages",
                    &s.local.console_search,
                    None,
                    |f| f.on_input(|s: &mut Studio, t| s.local.console_search = t.to_owned()),
                    |s: &mut Studio| s.local.console_search.clear(),
                )
                .grow(1.0),
            ),
        )
        .child(el().grow(1.0))
        .child(
            super::scene_overlays::toggle(
                l,
                icons::LAYERS,
                "Collapse repeated messages",
                s.local.console_collapse,
            )
            .key("console/collapse")
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "console.collapse", &crate::catalog::Target::None)
            }),
        )
        .child(
            super::scene_overlays::toggle(
                l,
                icons::ROTATE_CCW,
                "Clear on Play",
                s.editor
                    .state
                    .settings
                    .get("console.clearOnPlay")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            )
            .key("console/clear-on-play")
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "console.clear_on_play", &crate::catalog::Target::None)
            }),
        )
        .child(
            super::scene_overlays::toggle(
                l,
                icons::CIRCLE_PAUSE,
                "Pause Play on an error",
                s.editor
                    .state
                    .settings
                    .get("console.errorPause")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            )
            .key("console/error-pause")
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "console.error_pause", &crate::catalog::Target::None)
            }),
        )
        .child(
            button(l, Tone::Ghost, Size::Sm, Some(icons::TRASH), "Clear")
                .key("console/clear")
                .on_press(|s: &mut Studio| {
                    super::commands::run(s, "console.clear", &crate::catalog::Target::None)
                }),
        );
    let body = if shown.is_empty() {
        empty(
            l,
            icons::SQUARE_TERMINAL,
            "No messages",
            "Engine, script and agent messages appear here as they happen.",
            None,
        )
    } else {
        let rows: std::sync::Arc<Vec<LogRow>> = std::sync::Arc::new(shown);
        let n = rows.len();
        let look = l.clone();
        let open = s.local.console_open;
        virtual_list("console/rows", n, 34.0, move |range| {
            range
                .map(|i| log_row(&look, &rows[i], i % 2 == 1, open == Some(rows[i].seq)))
                .collect()
        })
        .h(0.0)
        .grow(1.0)
        .follow_end()
    };
    let detail = s
        .local
        .console_open
        .and_then(|seq| s.logs.iter().find(|r| r.seq == seq))
        .map(|r| log_detail(s, r));
    let machine_group = (!machine.is_empty()).then(|| {
        let c = &l.c;
        let open = s.local.console_machine;
        let hover = c.hover_surface.0;
        let mut group = col().child(
            row()
                .key("console/machine")
                .h(26.0)
                .pad_x(10.0)
                .gap(8.0)
                .align(Align::Center)
                .label("Driver and overlay messages")
                .tooltip("Messages from this computer's graphics drivers and overlay layers, not the project")
                .expanded(open)
                .hovered(move |e| e.fill(hover))
                .on_press(|s: &mut Studio| {
                    super::commands::run(s, "console.machine", &crate::catalog::Target::None)
                })
                .child(
                    icon(if open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT })
                        .size(11.0)
                        .color(c.muted_foreground.0),
                )
                .child(icon(icons::MONITOR).size(13.0).color(c.muted_foreground.0))
                .child(
                    text("Driver and overlay messages")
                        .text_size(12.0)
                        .color(c.muted_foreground.0)
                        .grow(1.0),
                )
                .child(count(l, machine.len(), false)),
        );
        if open {
            for (i, r) in machine.iter().enumerate() {
                group = group.child(log_row(l, r, i % 2 == 1, s.local.console_open == Some(r.seq)));
            }
        }
        group
    });
    col()
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar)
        .children(machine_group)
        .child(body)
        .children(detail.map(|d| col().child(d)))
}

/// The chosen record in full (Unity's console detail pane): the message, where it came
/// from, what its code means and why it usually happens, and its typed fixes.
fn log_detail(s: &Studio, r: &LogRow) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let doc = ara_diag::catalog::get(r.diagnostic.code);
    let mut head = row()
        .gap(8.0)
        .align(Align::Center)
        .child(severity_glyph(l, r.severity))
        .child(
            text(r.code.clone())
                .text_size(11.0)
                .family(mono())
                .color(c.muted_foreground.0),
        );
    if let Some(doc) = doc {
        head = head.child(
            text(doc.title)
                .text_size(12.0)
                .weight(600)
                .color(c.foreground.0),
        );
    }
    let copy = format!(
        "{} {}{}",
        r.code,
        r.message,
        r.source
            .as_deref()
            .map(|s| format!(" ({s})"))
            .unwrap_or_default()
    );
    head = head.child(el().grow(1.0)).child(
        icon_button(l, Tone::Ghost, Size::Sm, icons::COPY, "Copy message")
            .key("console/detail/copy")
            .on_press(move |s: &mut Studio| s.local.copy = Some(copy.clone())),
    );
    // Its source opens where the system opens it; a script in the IDE, at its line.
    if let Some(source) = r.source.as_deref() {
        let mut parts = source.split(':');
        let file = parts.next().unwrap_or(source).to_owned();
        let line = parts.next().and_then(|n| n.trim().parse::<u32>().ok());
        let column = parts.next().and_then(|n| n.trim().parse::<u32>().ok());
        if s.editor.host.root().join(&file).is_file() {
            head = head.child(
                icon_button(
                    l,
                    Tone::Ghost,
                    Size::Sm,
                    icons::EXTERNAL_LINK,
                    "Open source",
                )
                .key("console/detail/open")
                .on_press(move |s: &mut Studio| open_source(s, &file, line, column)),
            );
        }
    }
    head = head.child(
        icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Close details")
            .on_press(|s: &mut Studio| s.local.console_open = None),
    );
    let mut out = col()
        .max_h(if has_stack(&r.diagnostic) {
            300.0
        } else {
            180.0
        })
        .pad_x(12.0)
        .pad_y(8.0)
        .gap(6.0)
        .scroll(Axis::Vertical)
        .child(head)
        .child(
            text(r.message.clone())
                .text_size(12.0)
                .wrap(4)
                .color(c.foreground.0),
        );
    if let Some(source) = &r.source {
        out = out.child(
            row()
                .gap(6.0)
                .align(Align::Center)
                .child(icon(icons::FILE).size(12.0).color(c.muted_foreground.0))
                .child(
                    text(source.clone())
                        .text_size(11.0)
                        .family(mono())
                        .color(c.muted_foreground.0),
                ),
        );
    }
    if let Some(doc) = doc {
        out = out.child(
            super::style::prose(doc.summary())
                .text_size(11.5)
                .wrap(3)
                .color(c.muted_foreground.0),
        );
        if let Some(cause) = doc.causes.first() {
            out = out.child(
                super::style::prose(&format!("Usually: {cause}"))
                    .text_size(11.5)
                    .wrap(2)
                    .color(c.muted_foreground.0),
            );
        }
    }
    out = out.children(stack(s, &r.diagnostic));
    if !r.diagnostic.fixes.is_empty() {
        let mut fixes = row().gap(4.0);
        for (i, fix) in r.diagnostic.fixes.iter().enumerate().take(3) {
            let diagnostic = r.diagnostic.clone();
            fixes = fixes.child(
                button(
                    l,
                    Tone::Secondary,
                    Size::Sm,
                    Some(icons::WRENCH),
                    &fix.title,
                )
                .on_press(move |s: &mut Studio| {
                    if let Err(error) = crate::fixes::apply_fix(&mut s.editor, &diagnostic, i) {
                        s.editor
                            .report_failure(&std::io::Error::other(error.to_string()));
                    }
                    s.resolve();
                }),
            );
        }
        out = out.child(fixes);
    }
    out
}

fn level(severity: ara_diag::Severity) -> usize {
    match severity {
        ara_diag::Severity::Error => 0,
        ara_diag::Severity::Warning => 1,
        _ => 2,
    }
}

/// A console row: severity, the message, then its time, origin and source.
fn log_row(l: &ara_kit::look::Look, r: &LogRow, odd: bool, open: bool) -> El<Studio> {
    let c = &l.c;
    let secs = r.at / 1000;
    let meta = format!(
        "{:02}:{:02}:{:02}  ·  {}{}{}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
        r.origin,
        r.source
            .as_deref()
            .map(|s| format!("  ·  {s}"))
            .unwrap_or_default(),
        if r.count > 1 {
            format!("  ·  ×{}", r.count)
        } else {
            String::new()
        }
    );
    let seq = r.seq;
    let hover = c.hover_surface.0;
    row()
        .key(format!("console/row/{}", r.seq))
        .h(34.0)
        .pad_x(10.0)
        .gap(10.0)
        .align(Align::Center)
        .fill(if open {
            c.selection_surface_inactive.0
        } else if odd {
            c.muted.0.with_alpha(0.35)
        } else {
            stillkit::Rgba::TRANSPARENT
        })
        .selected(open)
        .hovered(move |e| if open { e } else { e.fill(hover) })
        .on_press(move |s: &mut Studio| {
            s.local.console_open = if s.local.console_open == Some(seq) {
                None
            } else {
                Some(seq)
            }
        })
        // Its menu (Copy, Collapse, Clear…), the message open in the details.
        .on_context_menu({
            let line = format!("{}: {}", r.code, r.message);
            move |s: &mut Studio, x, y| {
                s.local.console_open = Some(seq);
                s.local.context = Some((
                    crate::catalog::menus::Context::Console,
                    crate::catalog::Target::Text(line.clone()),
                    (x, y),
                ));
            }
        })
        .child(severity_glyph(l, r.severity))
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(r.message.clone())
                        .text_size(12.0)
                        .color(c.foreground.0),
                )
                .child(
                    text(meta)
                        .text_size(10.5)
                        .family(mono())
                        .color(c.muted_foreground.0),
                ),
        )
        .child(
            text(r.code.clone())
                .text_size(10.5)
                .family(mono())
                .color(c.muted_foreground.0),
        )
}

/// Problems: errors and warnings, grouped by file (or "Project" when they have none).
fn problems(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let all: Vec<&LogRow> = s
        .logs
        .iter()
        .filter(|r| r.severity != ara_diag::Severity::Info)
        .collect();
    let errors = all
        .iter()
        .filter(|r| r.severity == ara_diag::Severity::Error)
        .count();
    let toolbar = row()
        .h(super::style::TOOLBAR)
        .pad_x(8.0)
        .gap(4.0)
        .align(Align::Center)
        .child(
            level_toggle(
                l,
                "problems/level/0".into(),
                s.local.problems[0],
                c.destructive.0,
                icons::CIRCLE_X,
                "Errors",
                errors,
            )
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "problems.errors", &crate::catalog::Target::None)
            }),
        )
        .child(
            level_toggle(
                l,
                "problems/level/1".into(),
                s.local.problems[1],
                c.warning.0,
                icons::TRIANGLE_ALERT,
                "Warnings",
                all.len() - errors,
            )
            .on_press(|s: &mut Studio| {
                super::commands::run(s, "problems.warnings", &crate::catalog::Target::None)
            }),
        )
        .child(
            row().w(240.0).child(
                search(
                    l,
                    "problems/search",
                    "Search problems",
                    &s.local.problems_search,
                    None,
                    |f| f.on_input(|s: &mut Studio, t| s.local.problems_search = t.to_owned()),
                    |s: &mut Studio| s.local.problems_search.clear(),
                )
                .grow(1.0),
            ),
        );
    let query = s.local.problems_search.trim().to_lowercase();
    let shown = |r: &&&LogRow| {
        s.local.problems[usize::from(r.severity != ara_diag::Severity::Error)]
            && (query.is_empty()
                || r.message.to_lowercase().contains(&query)
                || r.code.to_lowercase().contains(&query)
                || r.source
                    .as_deref()
                    .is_some_and(|f| f.to_lowercase().contains(&query)))
    };
    let mut groups: std::collections::BTreeMap<&str, Vec<&LogRow>> =
        std::collections::BTreeMap::new();
    for r in all.iter().filter(shown) {
        groups
            .entry(
                r.source
                    .as_deref()
                    .map_or("Project", |s| s.split(':').next().unwrap_or(s)),
            )
            .or_default()
            .push(r);
    }
    let body = if all.is_empty() {
        empty(
            l,
            icons::CIRCLE_CHECK,
            "No problems",
            "Errors and warnings in the project's files and scenes show here, grouped by file.",
            None,
        )
    } else if groups.is_empty() {
        empty(
            l,
            icons::SEARCH,
            "Nothing matches",
            "Show errors and warnings again, or change the search.",
            None,
        )
    } else {
        let mut out = col().pad(8.0).gap(8.0);
        // Files with errors first, then the most problems, then by name.
        let mut groups: Vec<_> = groups.into_iter().collect();
        groups.sort_by_key(|(file, rows)| {
            let errors = rows
                .iter()
                .filter(|r| r.severity == ara_diag::Severity::Error)
                .count();
            (
                std::cmp::Reverse(errors),
                std::cmp::Reverse(rows.len()),
                *file,
            )
        });
        for (file, rows) in groups {
            let mut card = col()
                .radius(super::style::RADIUS)
                .fill(super::style::tones(l).band)
                .child(
                    row()
                        .h(30.0)
                        .pad_x(10.0)
                        .gap(8.0)
                        .align(Align::Center)
                        .child(icon(icons::FILE).size(13.0).color(c.muted_foreground.0))
                        .child(
                            text(file.to_owned())
                                .text_size(12.0)
                                .weight(600)
                                .color(c.foreground.0)
                                .grow(1.0),
                        )
                        .child(count(l, rows.len(), false)),
                );
            for r in rows {
                card = card.child(problem(s, r));
            }
            out = out.child(card);
        }
        col().grow(1.0).min_h(0.0).scroll(Axis::Vertical).child(out)
    };
    col().grow(1.0).min_h(0.0).child(toolbar).child(body)
}

/// One problem: its message and typed fixes (the same ones agents apply), Why? to read
/// what its code means and usually causes, and Go to for the entity it names.
fn problem(s: &Studio, r: &LogRow) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let open = s.local.problems_open.contains(&r.seq);
    let mut message = col().grow(1.0).min_w(0.0).gap(4.0).child(
        text(r.message.clone())
            .text_size(12.0)
            .wrap(3)
            .color(c.foreground.0),
    );
    if !r.diagnostic.fixes.is_empty() {
        let mut fixes = row().gap(4.0);
        for (i, fix) in r.diagnostic.fixes.iter().enumerate().take(2) {
            let diagnostic = r.diagnostic.clone();
            fixes = fixes.child(
                button(
                    l,
                    Tone::Secondary,
                    Size::Sm,
                    Some(icons::WRENCH),
                    &fix.title,
                )
                .on_press(move |s: &mut Studio| {
                    if let Err(error) = crate::fixes::apply_fix(&mut s.editor, &diagnostic, i) {
                        s.editor
                            .report_failure(&std::io::Error::other(error.to_string()));
                    }
                    s.resolve();
                }),
            );
        }
        message = message.child(fixes);
    }
    let doc = ara_diag::catalog::get(r.diagnostic.code);
    if open && let Some(doc) = doc {
        message = message
            .child(
                super::style::prose(&format!("{}: {}", doc.title, doc.summary()))
                    .text_size(11.5)
                    .wrap(4)
                    .color(c.muted_foreground.0),
            )
            .children(doc.causes.first().map(|cause| {
                super::style::prose(&format!("Usually: {cause}"))
                    .text_size(11.5)
                    .wrap(3)
                    .color(c.muted_foreground.0)
            }));
    }
    let seq = r.seq.to_string();
    let why = super::scene_overlays::toggle(
        l,
        icons::LIGHTBULB,
        "Why? What this means and usually causes",
        open,
    )
    .key(format!("problems/{}/why", r.seq))
    .on_press(move |s: &mut Studio| {
        super::commands::run(
            s,
            "problems.explain",
            &crate::catalog::Target::Text(seq.clone()),
        )
    });
    let location = &r.diagnostic.location;
    let go = location.record.clone().map(|record| {
        let scene = location.doc.clone();
        icon_button(
            l,
            Tone::Ghost,
            Size::Sm,
            icons::CROSSHAIR,
            "Go to: select and frame it",
        )
        .key(format!("problems/{}/go", r.seq))
        .on_press(move |s: &mut Studio| {
            if let Some(scene) = scene.as_ref().filter(|d| **d != s.editor.state.doc) {
                s.act("editor.scene.open", json!({"scene": scene}));
                s.resolve();
            }
            let entity = crate::catalog::Target::Entity(record.clone());
            super::commands::run(s, "edit.select_entity", &entity);
        })
    });
    row()
        .key(format!("problems/{}", r.seq))
        .min_h(32.0)
        .pad_x(10.0)
        .pad_y(6.0)
        .gap(10.0)
        .align(Align::Center)
        .child(severity_glyph(l, r.severity))
        .child(message)
        .child(
            text(r.code.clone())
                .text_size(10.5)
                .family(mono())
                .color(c.muted_foreground.0),
        )
        .children(doc.map(|_| why))
        .children(go)
}

/// Where a drop on the Hierarchy lands: under the row's entity, or at the scene's root on
/// its empty space (`Some(None)`); `None` off the Hierarchy.
fn hierarchy_at(s: &Studio, at: (f32, f32)) -> Option<Option<String>> {
    let inside =
        |r: &stillkit::Rect| at.0 >= r.left && at.0 <= r.right && at.1 >= r.top && at.1 <= r.bottom;
    let list = s.local.rects.get("hierarchy/rows").filter(|r| inside(r))?;
    let row = s.local.rects.iter().find_map(|(key, r)| {
        let id = key.strip_prefix("row/").filter(|id| !id.contains('/'))?;
        (inside(r) && inside(list)).then(|| id.to_owned())
    });
    Some(row)
}

/// The Inspector asset field under `at`, by path.
fn field_at(s: &Studio, at: (f32, f32)) -> Option<String> {
    s.local.rects.iter().find_map(|(key, r)| {
        let path = key.strip_prefix("field/")?;
        (at.0 >= r.left && at.0 <= r.right && at.1 >= r.top && at.1 <= r.bottom)
            .then(|| path.to_owned())
    })
}

/// The asset at storage `path` and its reference.
fn dropped(s: &Studio, path: &str) -> Option<(Asset, String)> {
    let asset = s
        .assets
        .as_ref()?
        .items
        .iter()
        .find(|a| a.path == path)
        .cloned()?;
    let reference = asset.current_reference(s.editor.host.store().workspace())?;
    Some((asset, reference))
}

/// Places the model or prefab at storage `path` in the scene under `parent` (the root
/// when `None`), as a drop on the Hierarchy does in Unity.
fn place_under(s: &mut Studio, path: &str, parent: Option<&str>) {
    let Some((asset, reference)) = dropped(s, path) else {
        return;
    };
    let mut args = match asset.kind.as_str() {
        "prefab" => json!({"prefab": reference}),
        _ => json!({"mesh": reference}),
    };
    if let Some(parent) = parent {
        args["parent"] = json!(parent);
    }
    let cmd = if asset.kind == "prefab" {
        "prefab.instantiate"
    } else {
        "editor.mesh.instantiate"
    };
    s.apply(vec![json!({"cmd": cmd, "args": args})], "Place asset");
}

/// Assigns the asset at storage `path` to Inspector field `field` on every inspected
/// entity, when the field takes that kind of asset (dropped on it).
fn assign(s: &mut Studio, path: &str, field: &str) {
    let Some(f) = super::inspector::field_of(s, field) else {
        return;
    };
    let crate::inspector::Widget::Asset { kind } = &f.widget else {
        return;
    };
    let Some((asset, reference)) = dropped(s, path) else {
        return;
    };
    if f.read_only || !asset.fits(kind.as_deref(), field) {
        super::note(s, format!("{} doesn't take {}", f.label, asset.name()));
        return;
    }
    let commands = crate::inspector::edit_commands(s.inspected(), field, &json!(reference));
    s.apply(commands, "Assign asset");
}

/// Places the model or prefab at storage `path` where the pointer let go: on the ground
/// under it in the Scene view (nothing happens elsewhere).
fn place(s: &mut Studio, path: &str, at: (f32, f32)) {
    let Some(view) = s.local.rects.get("scene/view").copied() else {
        return;
    };
    if !(at.0 >= view.left && at.0 <= view.right && at.1 >= view.top && at.1 <= view.bottom) {
        return;
    }
    let Some(asset) = s
        .assets
        .as_ref()
        .and_then(|b| b.items.iter().find(|a| a.path == path).cloned())
    else {
        return;
    };
    let Some(reference) = asset.current_reference(s.editor.host.store().workspace()) else {
        s.editor
            .report_failure(&std::io::Error::other("The asset is no longer available"));
        return;
    };
    let rect = ara_ui::paint::Box2 {
        x: 0.0,
        y: 0.0,
        w: view.width(),
        h: view.height(),
    };
    let ground = crate::overlay::ground_point(
        &s.editor.state.camera,
        rect,
        [at.0 - view.left, at.1 - view.top],
    )
    .unwrap_or(s.editor.state.camera.target);
    let position = json!([ground.x, ground.y, ground.z]);
    let command = if asset.kind == "prefab" {
        json!({"cmd": "prefab.instantiate", "args": {"prefab": reference, "position": position}})
    } else {
        json!({"cmd": "editor.mesh.instantiate", "args": {"mesh": reference, "position": position}})
    };
    s.apply(vec![command], "Place asset");
}
