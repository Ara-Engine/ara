//! The Build window (Unity's Build Settings, Unreal's Packaging): the project's build
//! profiles (`settings/builds/*.export.json`) to choose from, or one to create; Build,
//! Package in a chosen format, Run Build and Open Build Folder; the running export's stage
//! and cooking progress; the last build's folder and package files. Exports run on a
//! worker (`crate::build::Builder`, polled each pass by the host).
use ara_export::preset::PackageFormat;
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::json;
use stillkit::el::{Align, Axis, El, col, el, icon, row, text};

use super::Studio;
use crate::build::{PROFILE_SETTING, package_formats, profiles, selected_profile};

/// The project's build profiles, from the indexed project files.
fn names(s: &Studio) -> Vec<String> {
    s.assets
        .as_ref()
        .map(|b| profiles(&b.files))
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| crate::build::presets(s.editor.host.root()))
}

/// The profile Build uses now, if there is one.
fn chosen(s: &Studio, names: &[String]) -> Option<String> {
    let setting = s
        .editor
        .state
        .settings
        .get(PROFILE_SETTING)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    selected_profile(names, setting).ok().map(str::to_owned)
}

/// Runs a build action, reporting a failure where the editor reports them.
fn attempt(s: &mut Studio, run: impl FnOnce(&mut Studio) -> ara_ui::Result<()>) {
    if let Err(error) = run(s) {
        s.editor.report_failure(&*anyhow::anyhow!("{error}"));
    }
}

/// Builds the chosen profile (creating the default one when there is none).
pub(super) fn build(s: &mut Studio) {
    let names = names(s);
    attempt(s, |s| match chosen(s, &names) {
        Some(profile) => s.builder.build_profile(&mut s.editor, &profile, None),
        None => s.builder.build_current(&mut s.editor),
    });
}

/// Builds the chosen profile into `folder` (Build to Folder…), a fresh one the person
/// picked.
pub(super) fn build_into(s: &mut Studio, folder: std::path::PathBuf) -> ara_ui::Result<()> {
    let names = names(s);
    let profile = chosen(s, &names).ok_or("Create a build profile first")?;
    s.builder
        .build_profile(&mut s.editor, &profile, Some(folder))
}

/// Launches the last build.
pub(super) fn run(s: &mut Studio) {
    attempt(s, |s| s.builder.run_build(&mut s.editor, false));
}

/// Opens the last build's folder (or `builds/`).
pub(super) fn open_folder(s: &mut Studio) {
    let folder = s
        .builder
        .last_build
        .clone()
        .unwrap_or_else(|| s.editor.host.root().join("builds"));
    attempt(s, |_| {
        std::fs::create_dir_all(&folder)?;
        crate::system::open_external(&folder)
    });
}

/// Creates the portable default profile.
pub(super) fn create_profile(s: &mut Studio) {
    attempt(s, |s| {
        s.builder.create_build_profile(&mut s.editor).map(drop)
    });
}

/// The window.
pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let t = super::style::tones(l);
    let names = names(s);
    let selected = chosen(s, &names);
    let busy = s.builder.job.is_some();
    let band = |glyph, title: &str| {
        row()
            .h(28.0)
            .pad_x(10.0)
            .gap(8.0)
            .align(Align::Center)
            .fill(t.band)
            .child(icon(glyph).size(13.0).color(c.muted_foreground.0))
            .child(
                text(title.to_owned())
                    .text_size(12.5)
                    .weight(600)
                    .color(c.foreground.0),
            )
    };
    let mut profiles_list = col().gap(1.0).pad(6.0);
    if names.is_empty() {
        profiles_list = profiles_list.child(
            col().pad(10.0).gap(10.0).child(
                text("No build profile yet. Create the portable default: this platform, the real runtime player, a plain folder.")
                    .text_size(12.0)
                    .wrap(3)
                    .color(c.muted_foreground.0),
            ).child(
                row().child(
                    button(l, Tone::Primary, Size::Sm, Some(icons::FILE_PLUS), "Create build profile")
                        .key("build/create")
                        .on_press(create_profile),
                ),
            ),
        );
    }
    for name in &names {
        let on = selected.as_deref() == Some(name.as_str());
        let pick = name.clone();
        profiles_list = profiles_list.child(
            super::style::chosen(l, row(), on)
                .key(format!("build/profile/{name}"))
                .min_h(40.0)
                .pad_x(10.0)
                .pad_y(4.0)
                .gap(8.0)
                .align(Align::Center)
                .radius(super::style::RADIUS)
                .label(name.clone())
                .selected(on)
                .on_press(move |s: &mut Studio| {
                    s.act(
                        "editor.settings.set",
                        json!({"key": PROFILE_SETTING, "value": pick}),
                    )
                })
                .child(icon(icons::PACKAGE).size(13.0).color(c.muted_foreground.0))
                .child(
                    col()
                        .gap(2.0)
                        .w(0.0)
                        .grow(1.0)
                        .child(text(name.clone()).text_size(12.5).color(c.foreground.0))
                        .child(
                            text(format!("settings/builds/{name}.export.json"))
                                .text_size(10.5)
                                .family(mono())
                                .color(c.muted_foreground.0)
                                .wrap(1),
                        ),
                ),
        );
    }
    let left = col()
        .w(320.0)
        .min_w(240.0)
        .fill(t.panel)
        .child(band(icons::PACKAGE, "Profiles"))
        .child(profiles_list.scroll(Axis::Vertical).grow(1.0).h(0.0));
    let format_index = s.local.package_format.min(package_formats().len() - 1);
    let (format, format_title) = package_formats()[format_index];
    let actions = col()
        .pad(12.0)
        .gap(10.0)
        .child(
            row()
                .gap(8.0)
                .align(Align::Center)
                .child(enabled(
                    button(l, Tone::Primary, Size::Md, Some(icons::HAMMER), "Build")
                        .key("build/build")
                        .tooltip("Build the chosen profile into builds/ (Ctrl+Shift+B)")
                        .on_press(build),
                    !busy,
                ))
                .child(enabled(
                    button(l, Tone::Secondary, Size::Md, Some(icons::PLAY), "Run build")
                        .key("build/run")
                        .tooltip("Launch the last build, outside the editor")
                        .on_press(run),
                    !busy && s.builder.last_build.is_some(),
                ))
                .child(enabled(
                    button(
                        l,
                        Tone::Ghost,
                        Size::Md,
                        Some(icons::FOLDER),
                        "Build to folder…",
                    )
                    .key("build/destination")
                    .tooltip("Build the chosen profile into a folder you pick")
                    .on_press(|s: &mut Studio| {
                        super::commands::run(s, "build.destination", &crate::catalog::Target::None)
                    }),
                    !busy && selected.is_some(),
                ))
                .child(
                    button(
                        l,
                        Tone::Ghost,
                        Size::Md,
                        Some(icons::FOLDER_OPEN),
                        "Open build folder",
                    )
                    .key("build/folder")
                    .on_press(open_folder),
                ),
        )
        .child(package_row(s, format, format_title, selected.clone(), busy));
    let mut status = col().pad(12.0).gap(8.0);
    if let Some(job) = &s.builder.job {
        let fraction = job
            .cooking
            .map_or(0.0, |(done, total)| done as f32 / total.max(1) as f32);
        status = status
            .child(
                row()
                    .gap(8.0)
                    .align(Align::Center)
                    .child(spinner(l, 14.0))
                    .child(
                        text(format!("Building {} · {}", job.preset, job.stage))
                            .text_size(12.5)
                            .color(c.foreground.0),
                    ),
            )
            .children(job.cooking.map(|(done, total)| {
                col()
                    .gap(4.0)
                    .child(progress(l, fraction, Status::Info))
                    .child(
                        text(format!("Cooking {done} of {total}"))
                            .text_size(11.0)
                            .color(c.muted_foreground.0),
                    )
            }));
    } else if let Some(game) = &s.builder.last_build {
        status = status.child(
            row()
                .gap(8.0)
                .align(Align::Center)
                .child(el().size(8.0).radius(4.0).fill(c.success_foreground.0))
                .child(
                    text(format!("Last build: {}", game.display()))
                        .text_size(12.0)
                        .color(c.foreground.0)
                        .wrap(2),
                ),
        );
    } else {
        status = status.child(
            text("Not built yet in this session.")
                .text_size(12.0)
                .color(c.muted_foreground.0),
        );
    }
    for artifact in &s.builder.last_packages {
        let path = artifact.path.display().to_string();
        let reveal = path.clone();
        status = status.child(
            row()
                .key(format!("build/package/{path}"))
                .h(26.0)
                .gap(8.0)
                .align(Align::Center)
                .child(icon(icons::ARCHIVE).size(13.0).color(c.muted_foreground.0))
                .child(
                    text(path)
                        .text_size(11.5)
                        .family(mono())
                        .color(c.foreground.0)
                        .wrap(1)
                        .w(0.0)
                        .grow(1.0),
                )
                .child(
                    text(format!("{:.1} MB", artifact.bytes as f64 / 1_048_576.0))
                        .text_size(11.0)
                        .color(c.muted_foreground.0),
                )
                .child(
                    icon_button(l, Tone::Ghost, Size::Sm, icons::FOLDER_OPEN, "Reveal").on_press(
                        move |s: &mut Studio| {
                            let target = crate::catalog::Target::Text(reveal.clone());
                            attempt(s, |s| s.builder.reveal_package(&target, false));
                        },
                    ),
                ),
        );
    }
    let right = col()
        .grow(1.0)
        .min_w(0.0)
        .child(band(icons::HAMMER, selected.as_deref().unwrap_or("Build")))
        .child(actions)
        .child(start_row(s))
        .child(band(icons::ACTIVITY, "Status"))
        .child(status);
    row().grow(1.0).min_h(0.0).child(left).child(right)
}

/// Package: the format to package in, and the button that packages the chosen profile.
fn package_row(
    s: &Studio,
    format: PackageFormat,
    title: &'static str,
    profile: Option<String>,
    busy: bool,
) -> El<Studio> {
    let l = &s.look;
    const KEY: &str = "build/format";
    let open = s.local.select.as_deref() == Some(KEY);
    let mut trigger = select(l, Size::Md, KEY, title)
        .w(240.0)
        .expanded(open)
        .on_press(|s: &mut Studio| {
            s.local.select = match s.local.select.as_deref() == Some(KEY) {
                true => None,
                false => Some(KEY.into()),
            }
        });
    if open {
        let items = package_formats()
            .iter()
            .enumerate()
            .map(|(i, (_, name))| {
                menu_check(l, &format!("{KEY}/{i}"), name, *name == title).on_press(
                    move |s: &mut Studio| {
                        s.local.select = None;
                        s.local.package_format = i;
                    },
                )
            })
            .collect();
        trigger = trigger.overlay(
            stillkit::el::menu(menu_panel(l, items))
                .match_width()
                .on_dismiss(|s: &mut Studio| s.local.select = None),
        );
    }
    let can = !busy && profile.is_some();
    row()
        .gap(8.0)
        .align(Align::Center)
        .child(trigger)
        .child(enabled(
            button(
                l,
                Tone::Secondary,
                Size::Md,
                Some(icons::PACKAGE),
                "Package",
            )
            .key("build/package")
            .tooltip("Save the project, then export the chosen profile in this format")
            .on_press(move |s: &mut Studio| {
                let Some(profile) = profile.clone() else {
                    s.editor
                        .report_failure(&*anyhow::anyhow!("Create a build profile first"));
                    return;
                };
                attempt(s, |s| {
                    s.builder
                        .package_profile(&mut s.editor, &profile, None, format)
                });
            }),
            can,
        ))
}

/// The scene the project starts with (`ara.project.json`'s main), as written there.
pub(super) fn main_scene(s: &Studio) -> Option<String> {
    let workspace = s.editor.host.store().workspace();
    let settings = workspace
        .doc("pj_project")?
        .record("p_settings")?
        .get("settings")?;
    settings.get("main")?.as_str().map(str::to_owned)
}

/// Starts the project with scene document `doc`: one undoable `project.setMain`.
pub(super) fn set_main(s: &mut Studio, doc: &str) -> ara_ui::Result<()> {
    let workspace = s.editor.host.store().workspace();
    let path = workspace
        .path_of(doc)
        .ok_or("That scene is not in the project")?;
    let reference = format!("@{}#{doc}", workspace.reference_path(path));
    s.editor.apply_to(
        "pj_project",
        vec![serde_json::json!({"cmd": "project.setMain", "args": {"scene": reference}})],
        "Set the startup scene",
    )?;
    Ok(())
}

/// What the build starts with, and the way to make it the scene open now.
fn start_row(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let main = main_scene(s);
    let workspace = s.editor.host.store().workspace();
    let open = workspace
        .path_of(&s.editor.state.doc)
        .map(|p| workspace.reference_path(p).to_owned());
    let already = match (&main, &open) {
        (Some(main), Some(open)) => main == open || open.ends_with(main.as_str()),
        _ => false,
    };
    row()
        .key("build/start")
        .pad_x(12.0)
        .pad_y(8.0)
        .gap(8.0)
        .align(Align::Center)
        .child(icon(icons::PLAY).size(13.0).color(c.muted_foreground.0))
        .child(
            text("Starts with")
                .text_size(12.0)
                .color(c.muted_foreground.0),
        )
        .child(
            text(main.unwrap_or_else(|| "no scene set".to_owned()))
                .text_size(12.0)
                .family(mono())
                .color(c.foreground.0)
                .grow(1.0)
                .min_w(0.0),
        )
        .child(enabled(
            button(l, Tone::Ghost, Size::Sm, None, "Use open scene")
                .key("build/start/use-open")
                .tooltip("Start the project with the scene open now")
                .on_press(|s: &mut Studio| {
                    super::commands::run(s, "build.main_scene", &crate::catalog::Target::None)
                }),
            !already,
        ))
}
