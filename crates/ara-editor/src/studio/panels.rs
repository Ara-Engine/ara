//! The other panels: Assets (the project's files as a grid, folders first, breadcrumb and
//! search), Console (the diagnostics bus, newest last, severity toggles with counts) and
//! Problems (errors and warnings now, grouped by file). Panels not yet on stillkit say so
//! and name the legacy window that still has them.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::json;
use stillkit::el::{Align, Axis, El, Justify, col, el, icon, row, text, virtual_list};

use super::Studio;
use super::state::LogRow;
use crate::shell::assets::Asset;

pub(super) fn view(s: &Studio, panel: &str) -> El<Studio> {
    match panel {
        "assets" => assets(s),
        "console" => console(s),
        "problems" => problems(s),
        "agents" => super::agents::agents(s),
        "history" => super::agents::history(s),
        "lighting" => super::lighting::view(s),
        // Import settings live in the asset inspector; the Import tab shows the same.
        "import" => match s.editor.state.asset_path.as_deref() {
            Some(path) => super::asset_inspector::view(s, path),
            None => empty(
                &s.look,
                icons::DOWNLOAD,
                "Pick an asset",
                "Its importer settings, actions and dependencies show here and in the Inspector.",
                None,
            ),
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
                &format!("{title} is coming to the new editor"),
                "This panel still lives in the legacy window: run `ara editor --legacy-ui` to use it today.",
                None,
            )
        }
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
        |s: &mut Studio| s.act("editor.workspace", json!({"assetQuery": ""})),
    )
    .h(22.0)
    .w(200.0);
    let toolbar = row()
        .h(super::style::TOOLBAR)
        .pad_x(4.0)
        .gap(8.0)
        .align(Align::Center)
        .child(crumbs.grow(1.0))
        .child(search);
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
            if hidden {
                false
            } else if needle.is_empty() {
                parent(&a.path) == folder
            } else {
                a.kind != "folder" && a.path.to_lowercase().contains(&needle)
            }
        })
        .cloned()
        .collect();
    // Folders the scan lists only through their files show too.
    if needle.is_empty() {
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
                "Drop files onto the window to import them.",
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
                tile(
                    l,
                    &a,
                    selected.as_deref() == Some(a.path.as_str()),
                    preview,
                    held,
                )
            })
            .collect();
        col()
            .grow(1.0)
            .min_h(0.0)
            .scroll(Axis::Vertical)
            .child(row().flow_wrap().pad(6.0).gap(2.0).children(tiles))
    };
    // Unity's Project window: the folder tree beside the folder's contents.
    let right = col().w(0.0).grow(1.0).min_h(0.0).child(toolbar).child(body);
    row()
        .grow(1.0)
        .min_h(0.0)
        .child(folder_tree(s, browser, &folder))
        .child(right)
}

/// How many visible entries (files and subfolders) each folder holds directly.
fn contents(browser: &crate::shell::assets::Browser) -> std::collections::HashMap<String, usize> {
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
fn folder_tree(s: &Studio, browser: &crate::shell::assets::Browser, current: &str) -> El<Studio> {
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
    tree = tree.child(folder_row(
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
        .on_press(move |s: &mut Studio| s.act("editor.workspace", json!({"folder": open})))
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
        .on_press(move |s: &mut Studio| s.act("editor.workspace", json!({"folder": path})))
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
) -> El<Studio> {
    let c = &l.c;
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
    super::style::chosen(l, col(), selected)
        .key(format!("asset/{}", a.path))
        .w(84.0)
        .pad_x(4.0)
        .pad_y(6.0)
        .gap(3.0)
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
                use crate::shell::commands::{Target, menus::Context};
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
                    PointerPhase::Down if e.clicks == 2 && folder => {
                        s.act("editor.workspace", json!({"folder": path}))
                    }
                    PointerPhase::Down if e.clicks == 2 && scene => {
                        if let Some(doc) = &reference {
                            s.act("editor.scene.open", json!({"scene": doc}));
                            s.resolve();
                        }
                    }
                    PointerPhase::Down => {
                        s.act("editor.workspace", json!({"asset": path}));
                        s.local.inspect_asset = !folder;
                    }
                    // Models and prefabs drag into the Scene.
                    PointerPhase::Move
                        if placeable && e.travel().x.abs() + e.travel().y.abs() >= 4.0 =>
                    {
                        s.local.asset_drag = Some((path.clone(), name.clone(), (e.at.x, e.at.y)));
                    }
                    PointerPhase::Up => {
                        if let Some((path, _, at)) = s.local.asset_drag.take() {
                            place(s, &path, at);
                        }
                    }
                    PointerPhase::Cancel => s.local.asset_drag = None,
                    _ => return false,
                }
                true
            }
        })
        .child(match preview {
            // The rendered picture, or the kind's bare icon until it lands.
            Some(picture) => stillkit::el::image(picture).size(52.0),
            None => el()
                .size(52.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .child(
                    icon(glyph)
                        .size(if folder { 44.0 } else { 36.0 })
                        .color(glyph_ink),
                ),
        })
        .child(
            text(stem.to_owned())
                .text_size(11.5)
                .wrap(2)
                .text_align(stillkit::el::TextAlign::Center)
                .color(c.foreground.0),
        )
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
    let shown: Vec<&LogRow> = s
        .logs
        .iter()
        .filter(|r| s.local.console[level(r.severity)])
        .collect();
    // Severity filters: the icon in its colour and the count, a soft fill while shown.
    let c = &l.c;
    let toggle = |i: usize, glyph, label: &str| {
        let on = s.local.console[i];
        let tint = [c.destructive.0, c.warning.0, c.info.0][i];
        let hover = c.hover_surface.0;
        row()
            .key(format!("console/level/{i}"))
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
            .label(format!("{label}: {}", counts[i]))
            .tooltip(format!("{} {label}", if on { "Hide" } else { "Show" }))
            .checked(on)
            .hovered(move |e| e.fill(hover))
            .on_press(move |s: &mut Studio| s.local.console[i] = !s.local.console[i])
            .child(
                icon(glyph)
                    .size(13.0)
                    .color(if on { tint } else { tint.with_alpha(0.45) }),
            )
            .child(text(counts[i].to_string()).text_size(12.0).weight(560))
    };
    let toolbar = row()
        .h(super::style::TOOLBAR)
        .pad_x(8.0)
        .gap(4.0)
        .align(Align::Center)
        .child(toggle(0, icons::CIRCLE_X, "Errors"))
        .child(toggle(1, icons::TRIANGLE_ALERT, "Warnings"))
        .child(toggle(2, icons::INFO, "Messages"))
        .child(el().grow(1.0))
        .child(
            button(l, Tone::Ghost, Size::Sm, Some(icons::TRASH), "Clear")
                .key("console/clear")
                .on_press(|s: &mut Studio| s.logs.clear()),
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
        let rows: std::sync::Arc<Vec<LogRow>> =
            std::sync::Arc::new(shown.into_iter().cloned().collect());
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
    col()
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar)
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
    head = head.child(el().grow(1.0)).child(
        icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Close details")
            .on_press(|s: &mut Studio| s.local.console_open = None),
    );
    let mut out = col()
        .max_h(180.0)
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
            text(doc.summary())
                .text_size(11.5)
                .wrap(3)
                .color(c.muted_foreground.0),
        );
        if let Some(cause) = doc.causes.first() {
            out = out.child(
                text(format!("Usually: {cause}"))
                    .text_size(11.5)
                    .wrap(2)
                    .color(c.muted_foreground.0),
            );
        }
    }
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
                    if let Err(error) =
                        crate::shell::input::apply_fix(&mut s.editor, &diagnostic, i)
                    {
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
    let mut groups: std::collections::BTreeMap<&str, Vec<&LogRow>> =
        std::collections::BTreeMap::new();
    for r in s
        .logs
        .iter()
        .filter(|r| r.severity != ara_diag::Severity::Info)
    {
        groups
            .entry(
                r.source
                    .as_deref()
                    .map_or("Project", |s| s.split(':').next().unwrap_or(s)),
            )
            .or_default()
            .push(r);
    }
    if groups.is_empty() {
        return empty(
            l,
            icons::CIRCLE_CHECK,
            "No problems",
            "Errors and warnings in the project's files and scenes show here, grouped by file.",
            None,
        );
    }
    let mut out = col().pad(8.0).gap(8.0);
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
            // Its typed fixes, one click each (the same ones agents apply).
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
                            if let Err(error) =
                                crate::shell::input::apply_fix(&mut s.editor, &diagnostic, i)
                            {
                                s.editor
                                    .report_failure(&std::io::Error::other(error.to_string()));
                            }
                            s.resolve();
                        }),
                    );
                }
                message = message.child(fixes);
            }
            card = card.child(
                row()
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
                    ),
            );
        }
        out = out.child(card);
    }
    col().grow(1.0).min_h(0.0).scroll(Axis::Vertical).child(out)
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
    let ground = crate::shell::overlay::ground_point(
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
