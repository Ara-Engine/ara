//! The Inspector for an asset, as Unity shows one: picking a file in Assets turns the
//! Inspector to it until an entity is picked again. A header (preview, name, path and
//! importer), its importer settings as typed rows (each edit is the shared
//! `import.configure` path, one undo step), Reimport or Import, Open for graphs and
//! animations, and what it uses and what uses it (each a link to that asset).
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::el::{Align, Axis, El, Side, col, el, icon, image, menu, row, text};

use super::Studio;
use super::style::ROW;
use crate::shell::import_batch::{self, Change};

/// The asset inspector for `path`.
pub(super) fn view(s: &Studio, path: &str) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let store = s.editor.host.store();
    let workspace = store.workspace();
    let asset = s
        .assets
        .as_ref()
        .and_then(|b| b.items.iter().find(|a| a.path == path));
    let name = path.rsplit('/').next().unwrap_or(path);
    let logical = asset.and_then(|a| a.authored_path.as_deref());
    // Undoing an import leaves the sidecar without settings: that reads as not imported.
    let imported = logical.and_then(|source| {
        crate::asset_tools::paths::import_document(workspace, source)
            .and_then(|doc| doc.record("i_settings").map(|record| (doc, record)))
    });
    let importer = imported
        .as_ref()
        .and_then(|(_, record)| record.get("importer"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let preview = match s.previews.images.get(path) {
        Some(picture) => image(picture.clone())
            .size(64.0)
            .radius(super::style::RADIUS),
        None => el()
            .size(64.0)
            .radius(super::style::RADIUS)
            .fill(super::style::tones(l).well)
            .align(Align::Center)
            .justify(stillkit::el::Justify::Center)
            .child(icon(icons::FILE).size(24.0).color(c.muted_foreground.0)),
    };
    let header = row()
        .pad(10.0)
        .gap(10.0)
        .align(Align::Start)
        .child(preview)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(3.0)
                .child(text(name).text_size(13.5).weight(600).color(c.foreground.0))
                .child(
                    text(path)
                        .text_size(11.0)
                        .color(c.muted_foreground.0)
                        .wrap(2),
                )
                .children((!importer.is_empty()).then(|| {
                    row()
                        .pad_y(2.0)
                        .child(chip(l, Status::Info, importer, false))
                })),
        );
    let mut body = col().gap(2.0).child(header);
    if let Some((doc, record)) = &imported {
        body = body.child(band(l, "Import settings"));
        match settings_rows(s, doc.id.as_str(), record) {
            Ok(rows) => body = body.children(rows),
            Err(error) => {
                body = body.child(
                    text(error.to_string())
                        .text_size(11.5)
                        .color(c.destructive_foreground.0)
                        .pad(10.0),
                )
            }
        }
        let p = path.to_owned();
        body = body.child(
            row().pad(10.0).child(
                button(
                    l,
                    Tone::Secondary,
                    Size::Sm,
                    Some(icons::REFRESH_CW),
                    "Reimport",
                )
                .key("asset/reimport")
                .on_press(move |s: &mut Studio| s.act("editor.asset.reimport", json!({"path": p}))),
            ),
        );
    } else if logical.is_some() && ara_importers::commands::importer_for_source(path).is_ok() {
        let p = path.to_owned();
        body = body.child(
            col()
                .pad(10.0)
                .gap(8.0)
                .child(
                    text("Not imported yet: create its settings and cook it in a worker.")
                        .text_size(11.5)
                        .color(c.muted_foreground.0)
                        .wrap(3),
                )
                .child(
                    row().child(
                        button(l, Tone::Primary, Size::Sm, Some(icons::DOWNLOAD), "Import")
                            .key("asset/import")
                            .on_press(move |s: &mut Studio| {
                                s.act("editor.asset.import", json!({"path": p}))
                            }),
                    ),
                ),
        );
    }
    if let Some(asset) = asset
        && matches!(asset.kind.as_str(), "graph" | "animation")
        && let Some(doc) = crate::asset_links::namespace::document(workspace, asset)
    {
        let id = doc.id.to_string();
        body = body.child(
            row().pad_x(10.0).pad_y(4.0).child(
                button(
                    l,
                    Tone::Primary,
                    Size::Sm,
                    Some(icons::EXTERNAL_LINK),
                    "Open editor",
                )
                .key("asset/open")
                .on_press(move |s: &mut Studio| s.act("editor.workspace", json!({"resource": id}))),
            ),
        );
    }
    // A scene opens in the Scene view.
    if let Some(reference) = asset
        .filter(|a| a.path.ends_with(".scene"))
        .and_then(|a| a.reference.clone())
    {
        body = body.child(
            row().pad_x(10.0).pad_y(4.0).child(
                button(
                    l,
                    Tone::Primary,
                    Size::Sm,
                    Some(icons::EXTERNAL_LINK),
                    "Open scene",
                )
                .key("asset/open-scene")
                .on_press(move |s: &mut Studio| {
                    s.act("editor.scene.open", json!({"scene": reference}));
                    s.resolve();
                }),
            ),
        );
    }
    if let Some(browser) = &s.assets {
        let (uses, used_by) = crate::asset_links::links(browser, workspace, path);
        for (title, links) in [("Uses", uses), ("Used by", used_by)] {
            body = body.child(band(l, &format!("{title} · {}", links.len())));
            if links.is_empty() {
                body = body.child(
                    row()
                        .pad_x(12.0)
                        .pad_y(4.0)
                        .child(text("Nothing").text_size(11.5).color(c.muted_foreground.0)),
                );
            }
            for link in links {
                let shown = workspace.reference_path(&link).to_owned();
                let hover = c.hover_surface.0;
                body = body.child(
                    row()
                        .key(format!("asset/link/{title}/{link}"))
                        .h(ROW + 2.0)
                        .pad_x(8.0)
                        .gap(8.0)
                        .align(Align::Center)
                        .radius(super::style::RADIUS)
                        .color(c.foreground.0)
                        .cascade_ink()
                        .label(shown.clone())
                        .hovered(move |e| e.fill(hover))
                        .on_press(move |s: &mut Studio| {
                            s.act("editor.workspace", json!({"asset": link}))
                        })
                        .child(icon(icons::FILE).size(13.0).color(c.muted_foreground.0))
                        .child(text(shown).text_size(12.0).grow(1.0)),
                );
            }
        }
    }
    col()
        .key("asset/inspector")
        .h(0.0)
        .grow(1.0)
        .scroll(Axis::Vertical)
        .child(body)
}

/// A section heading, as component headers are.
fn band(l: &ara_kit::look::Look, title: &str) -> El<Studio> {
    row()
        .h(26.0)
        .pad_x(10.0)
        .align(Align::Center)
        .fill(super::style::tones(l).band)
        .child(
            text(title.to_owned())
                .text_size(12.0)
                .weight(600)
                .color(l.c.foreground.0),
        )
}

/// The importer's settings as rows: each field with its schema doc, a checkbox, a choice or
/// a typed value. Values the sidecar leaves out show the project's inherited defaults.
fn settings_rows(
    s: &Studio,
    doc: &str,
    record: &ara_doc::Record,
) -> ara_ui::Result<Vec<El<Studio>>> {
    let l = &s.look;
    let c = &l.c;
    let settings: ara_asset::ImportSettings =
        serde_json::from_value(Value::Object(record.clone()))?;
    let (defaults, schema) = match settings.importer.as_str() {
        "ara.texture" => (
            ara_json::to_value(&ara_importers::TextureSettings::default())?,
            "TextureSettings",
        ),
        "ara.audio" => (
            ara_json::to_value(&ara_importers::audio::Settings::default())?,
            "AudioImportSettings",
        ),
        _ => (
            ara_json::to_value(&ara_importers::MeshSettings::default())?,
            "MeshSettings",
        ),
    };
    let defaults = import_batch::inherited_defaults(&s.editor, &settings.importer, defaults)?;
    let db = s.editor.host.store().schema();
    let metadata = db.get(schema).and_then(|ty| match &ty.shape {
        ara_schema::Shape::Struct { fields, .. } => Some(fields),
        _ => None,
    });
    let mut out = Vec::new();
    let Some(fields) = defaults.as_object() else {
        return Ok(out);
    };
    for (key, default) in fields {
        // Encoding quality follows the project; FBX opacity maps are not a choice.
        if key == "preset" || (settings.importer == "ara.fbx" && key == "opacityMaps") {
            continue;
        }
        let value = settings.settings.get(key).unwrap_or(default).clone();
        let desc = metadata.and_then(|fields| fields.iter().find(|f| f.name == *key));
        let help = desc.map(|f| f.doc.clone()).unwrap_or_default();
        let name = match key.as_str() {
            "lodCount" => "LOD count".to_owned(),
            "lodError" => "LOD error".to_owned(),
            "flipNormalY" => "Flip normal Y".to_owned(),
            "maxDimension" => "Max dimension".to_owned(),
            _ => crate::names::sentence(key),
        };
        let field_key = format!("asset/setting/{key}");
        let (doc_id, setting) = (doc.to_owned(), key.clone());
        let editor: El<Studio> = if let Some(on) = value.as_bool() {
            row().child(
                checkbox(l, field_key, &name, on)
                    .on_press(move |s: &mut Studio| set(s, &doc_id, &setting, json!(!on))),
            )
        } else if let Some(crate::inspector::Widget::Enum { options }) =
            desc.map(|d| crate::inspector::widget(db, d))
        {
            let nullable = desc.is_some_and(|d| matches!(&d.ty, ara_schema::ValueType::Option(_)));
            let choices = import_batch::enum_values(options, nullable);
            let current = choices
                .iter()
                .find(|(v, _)| *v == value)
                .map_or_else(|| value.to_string(), |(_, label)| label.clone());
            let open = s.local.select.as_deref() == Some(field_key.as_str());
            let k = field_key.clone();
            let mut trigger = select(l, Size::Sm, field_key.clone(), &current)
                .expanded(open)
                .on_press(move |s: &mut Studio| {
                    s.local.select = if s.local.select.as_deref() == Some(k.as_str()) {
                        None
                    } else {
                        Some(k.clone())
                    };
                });
            if open {
                let items = choices
                    .into_iter()
                    .map(|(v, label)| {
                        let (doc_id, setting) = (doc_id.clone(), setting.clone());
                        menu_check(l, &format!("{field_key}/{label}"), &label, v == value).on_press(
                            move |s: &mut Studio| {
                                s.local.select = None;
                                set(s, &doc_id, &setting, v.clone());
                            },
                        )
                    })
                    .collect();
                trigger = trigger.overlay(
                    menu(menu_panel(l, items))
                        .side(Side::Bottom)
                        .match_width()
                        .on_dismiss(|s: &mut Studio| s.local.select = None),
                );
            }
            trigger
        } else {
            let shown = match &value {
                Value::Object(o) if o.is_empty() => String::new(),
                Value::Array(a) if a.is_empty() => String::new(),
                Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            let number = value.is_number();
            field(l, Size::Sm, field_key, None, "", &shown, move |x| {
                x.on_submit(move |s: &mut Studio, t| {
                    let typed = if number {
                        t.trim().parse::<f64>().map(|n| json!(n)).ok()
                    } else {
                        Some(serde_json::from_str(t).unwrap_or_else(|_| json!(t)))
                    };
                    if let Some(v) = typed {
                        set(s, &doc_id, &setting, v);
                    }
                })
            })
        };
        out.push(
            row()
                .min_h(ROW + 4.0)
                .pad_x(10.0)
                .gap(8.0)
                .align(Align::Center)
                .child(
                    text(name.clone())
                        .text_size(12.0)
                        .color(c.muted_foreground.0)
                        .w(0.0)
                        .grow(0.42)
                        .tooltip(help),
                )
                .child(editor.w(0.0).grow(0.58)),
        );
    }
    Ok(out)
}

/// Sets one importer setting (one undo step through `import.configure`).
fn set(s: &mut Studio, doc: &str, field: &str, value: Value) {
    let change = Change::Setting {
        doc: doc.to_owned(),
        field: field.to_owned(),
        value,
    };
    if let Err(error) =
        import_batch::apply_changes(&mut s.editor, vec![change], "Edit importer settings")
    {
        s.editor.report_failure(&*error);
    }
}
