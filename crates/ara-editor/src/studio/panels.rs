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
    .h(24.0)
    .w(200.0);
    let toolbar = row()
        .h(32.0)
        .pad_x(6.0)
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
        let tiles: Vec<El<Studio>> = items
            .into_iter()
            .map(|a| tile(l, &a, selected.as_deref() == Some(a.path.as_str())))
            .collect();
        col()
            .grow(1.0)
            .min_h(0.0)
            .scroll(Axis::Vertical)
            .child(row().flow_wrap().pad(8.0).gap(4.0).children(tiles))
    };
    // Unity's Project window: the folder tree beside the folder's contents.
    let right = col()
        .w(0.0)
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar)
        .child(divider(l))
        .child(body);
    row()
        .grow(1.0)
        .min_h(0.0)
        .child(folder_tree(s, browser, &folder))
        .child(el().w(1.0).h_full().fill(c.border.0))
        .child(right)
}

/// Every folder in the project, as a tree: the open ones and the current one's ancestors
/// show their children. A click opens the folder in the grid.
fn folder_tree(s: &Studio, browser: &crate::shell::assets::Browser, current: &str) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
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
        icons::BOX,
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
        let glyph = if current == dir {
            icons::FOLDER_OPEN
        } else {
            icons::FOLDER
        };
        tree = tree.child(folder_row(
            l,
            name,
            dir.clone(),
            depth,
            current == dir,
            expanded,
            glyph,
        ));
    }
    tree.fill(c.muted.0.with_alpha(0.35))
}

/// A folder tree row: disclosure (when it has subfolders), icon and name.
fn folder_row(
    l: &ara_kit::look::Look,
    name: &str,
    path: String,
    depth: usize,
    current: bool,
    expanded: Option<bool>,
    glyph: stillkit::icons::Icon,
) -> El<Studio> {
    let c = &l.c;
    let hover = c.hover_surface.0;
    let (fill, ink) = if current {
        (c.selection_surface.0, c.selection_foreground.0)
    } else {
        (stillkit::Rgba::TRANSPARENT, c.foreground.0)
    };
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
    row()
        .key(format!("assets/tree/{path}"))
        .h(22.0)
        .pad_x(4.0)
        .gap(4.0)
        .align(Align::Center)
        .radius(4.0)
        .fill(fill)
        .color(ink)
        .cascade_ink()
        .label(name.to_owned())
        .selected(current)
        .hovered(move |e| if current { e } else { e.fill(hover) })
        .on_press(move |s: &mut Studio| s.act("editor.workspace", json!({"folder": open})))
        .child(el().w(depth as f32 * 12.0))
        .child(disclosure)
        .child(
            icon(glyph)
                .size(13.0)
                .color(if current { ink } else { c.warning.0 }),
        )
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

/// One asset: its icon on a soft plate and the name under it, two lines at most.
fn tile(l: &ara_kit::look::Look, a: &Asset, selected: bool) -> El<Studio> {
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
    let reference = a.authored_path.clone();
    let glyph_ink = if folder {
        c.warning.0
    } else {
        c.muted_foreground.0
    };
    let hover = c.hover_surface.0;
    col()
        .key(format!("asset/{}", a.path))
        .w(84.0)
        .pad(6.0)
        .gap(5.0)
        .align(Align::Center)
        .radius(l.radius.sm)
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
        .on_pointer(move |s: &mut Studio, e, _| {
            if e.phase != stillkit::el::PointerPhase::Down || e.button != stillkit::Button::Left {
                return false;
            }
            if e.clicks == 2 && folder {
                s.act("editor.workspace", json!({"folder": path}));
            } else if e.clicks == 2 && scene {
                if let Some(doc) = &reference {
                    s.act("editor.open", json!({"doc": doc}));
                    s.resolve();
                }
            } else {
                s.act("editor.workspace", json!({"asset": path}));
            }
            true
        })
        .child(
            el().size(52.0)
                .radius(l.radius.sm)
                .fill(c.raised.0)
                .border(1.0, c.border.0)
                .align(Align::Center)
                .justify(Justify::Center)
                .child(icon(kind_icon(a)).size(24.0).color(glyph_ink)),
        )
        .child(
            text(stem.to_owned())
                .text_size(11.5)
                .wrap(2)
                .text_align(stillkit::el::TextAlign::Center)
                .color(if selected {
                    c.selection_foreground.0
                } else {
                    c.foreground.0
                }),
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
            .border(
                1.0,
                if on {
                    c.border.0
                } else {
                    stillkit::Rgba::TRANSPARENT
                },
            )
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
        .h(30.0)
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
        virtual_list("console/rows", n, 34.0, move |range| {
            range
                .map(|i| log_row(&look, &rows[i], i % 2 == 1))
                .collect()
        })
        .grow(1.0)
        .follow_end()
    };
    col()
        .grow(1.0)
        .min_h(0.0)
        .child(toolbar)
        .child(divider(l))
        .child(body)
}

fn level(severity: ara_diag::Severity) -> usize {
    match severity {
        ara_diag::Severity::Error => 0,
        ara_diag::Severity::Warning => 1,
        _ => 2,
    }
}

/// A console row: severity, the message, then its time, origin and source.
fn log_row(l: &ara_kit::look::Look, r: &LogRow, odd: bool) -> El<Studio> {
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
    row()
        .h(34.0)
        .pad_x(10.0)
        .gap(10.0)
        .align(Align::Center)
        .fill(if odd {
            c.muted.0.with_alpha(0.35)
        } else {
            stillkit::Rgba::TRANSPARENT
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
        let mut card = card(l).fill(c.muted.0).child(
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
            card = card.child(divider(l)).child(
                row()
                    .min_h(32.0)
                    .pad_x(10.0)
                    .pad_y(6.0)
                    .gap(10.0)
                    .align(Align::Center)
                    .child(severity_glyph(l, r.severity))
                    .child(
                        text(r.message.clone())
                            .text_size(12.0)
                            .wrap(3)
                            .color(c.foreground.0)
                            .grow(1.0)
                            .min_w(0.0),
                    )
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
