//! Native build-profile selection and optional output-envelope prompt.
//! The worker's semantic file index supplies profiles; build.rs owns exporting.
use super::*;
use ara_export::preset::PackageFormat;
use ara_host::project::{AuthoredFile, AuthoredRole, AuthoredScope};
use ara_ui::{
    dialogs::{DialogFuture, DialogOptions},
    input::InputEvent,
};
use std::{collections::BTreeMap, path::PathBuf};

/// Typed preference shared by the Settings form, profile select and build commands.
pub(crate) const PROFILE_SETTING: &str = "build.profile";
const ROOT: &str = "build-options";
const CONTROL: &str = "build-profile-select";

struct Pending {
    profile: String,
    format: Option<PackageFormat>,
    response: DialogFuture<Option<String>>,
}

/// Transient form/prompt state. The selected profile itself is a typed setting.
#[derive(Default)]
pub(crate) struct State {
    open: bool,
    package: bool,
    format: Option<PackageFormat>,
    pending: Option<Pending>,
}

/// Profile names accepted by the build API, resolved from cached project entries.
/// Canonical settings/builds entries take precedence over indexed legacy presets.
pub(crate) fn profiles(files: &[AuthoredFile]) -> Vec<String> {
    let mut names = BTreeMap::new();
    for file in files {
        if !matches!(&file.scope, AuthoredScope::Project)
            || !matches!(&file.role, AuthoredRole::BuildPreset)
        {
            continue;
        }
        let Some((name, canonical)) = profile_name(&file.storage_path) else {
            continue;
        };
        names
            .entry(name.to_owned())
            .and_modify(|existing: &mut bool| *existing |= canonical)
            .or_insert(canonical);
    }
    names.into_keys().collect()
}

fn profile_name(storage: &str) -> Option<(&str, bool)> {
    let (name, canonical) = storage
        .strip_prefix("settings/builds/")
        .map(|name| (name, true))
        .or_else(|| storage.strip_prefix("presets/").map(|name| (name, false)))?;
    let name = name.strip_suffix(".export.json")?;
    (!name.is_empty() && !name.contains(['/', '\\']) && !name.chars().any(char::is_control))
        .then_some((name, canonical))
}

impl Shell {
    /// Cached profiles for the dynamic Build menu and searchable select.
    pub(crate) fn build_profiles(&self) -> Vec<String> {
        profiles(self.asset_authored_files())
    }

    /// Explicit selection wins; empty uses the portable default if present, then
    /// the first cached profile. A removed explicit selection is never substituted.
    pub(crate) fn selected_build_profile(&self, editor: &Editor) -> ara_ui::Result<String> {
        let names = self.build_profiles();
        let selected = editor
            .state
            .settings
            .get(PROFILE_SETTING)
            .and_then(Value::as_str)
            .unwrap_or("");
        selected_profile(&names, selected).map(str::to_owned)
    }

    /// Select from a dynamic menu through the same typed preference as the form.
    pub(crate) fn select_build_profile(
        &mut self,
        editor: &mut Editor,
        profile: &str,
    ) -> ara_ui::Result<()> {
        if !self.build_profiles().iter().any(|name| name == profile) {
            return Err("That build profile is no longer in the project index".into());
        }
        editor.action(
            "editor.settings.set",
            json!({"key":PROFILE_SETTING,"value":profile}),
        )?;
        self.rebuild(editor)
    }

    pub(crate) fn build_options_open(&self) -> bool {
        self.build_options.open && self.current.is_none()
    }

    /// Open an input-trapping shared-kit modal without inspecting project files.
    pub(crate) fn open_build_options(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.open_export_options(editor, false)
    }

    /// Choose a real export package format, using the existing profile/output form.
    pub(crate) fn open_package_options(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.open_export_options(editor, true)
    }

    fn open_export_options(&mut self, editor: &mut Editor, package: bool) -> ara_ui::Result<()> {
        if self.dialog.is_some() || self.confirmations.is_open() {
            return Err(
                "Finish or close the current dialog, then open Build Profiles again".into(),
            );
        }
        if self.build_options.pending.is_some() {
            return Ok(());
        }
        if editor.state.palette {
            editor.action("editor.palette", json!({"open":false}))?;
        }
        self.leave();
        self.menu = None;
        self.build_options.open = true;
        self.build_options.package = package;
        self.rebuild(editor)
    }

    pub(super) fn choose_package_format(&mut self, value: Value) -> ara_ui::Result<()> {
        if !self.build_options.open || !self.build_options.package {
            return Err("The package form is no longer open".into());
        }
        let format = serde_json::from_value(value)
            .map_err(|error| format!("Choose a supported package format: {error}"))?;
        self.build_options.format = Some(format);
        Ok(())
    }

    /// Lower the reviewed profile and format to the existing nonblocking worker.
    pub(crate) fn package_selected(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if !self.build_options.open || !self.build_options.package {
            return self.open_package_options(editor);
        }
        let profile = self.selected_build_profile(editor)?;
        let format = self
            .build_options
            .format
            .unwrap_or(PackageFormat::WindowsPortable);
        self.package_profile(editor, &profile, None, format)?;
        self.build_options.open = false;
        self.rebuild(editor)
    }

    pub(crate) fn close_build_options(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.build_options.open = false;
        self.rebuild(editor)
    }

    /// Build the chosen profile into the backend's fresh, uniquely named envelope.
    pub(crate) fn build_selected(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let profile = match self.selected_build_profile(editor) {
            Ok(profile) => profile,
            Err(_)
                if self.build_profiles().is_empty()
                    && editor
                        .state
                        .settings
                        .get(PROFILE_SETTING)
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .is_empty() =>
            {
                return self.build_current(editor);
            }
            Err(_) => return self.open_build_options(editor),
        };
        self.build_profile(editor, &profile, None)?;
        self.build_options.open = false;
        self.rebuild(editor)
    }

    /// Capture the selected profile while the optional output prompt is open;
    /// background preference changes cannot redirect an accepted build.
    pub(crate) fn prompt_build_destination(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self.build_options.pending.is_some() {
            return Ok(());
        }
        let profile = self.selected_build_profile(editor)?;
        let format = (self.build_options.open && self.build_options.package).then(|| {
            self.build_options
                .format
                .unwrap_or(PackageFormat::WindowsPortable)
        });
        let mut options = DialogOptions::new(
            format!(
                "{} {profile}",
                if format.is_some() { "Package" } else { "Build" }
            ),
            "Enter a new output folder. Leave this empty to let Ara generate a fresh folder under builds for this profile. Existing build folders are preserved.",
        );
        options.accept_label = if format.is_some() { "Package" } else { "Build" }.into();
        let response = self.confirmations.prompt(options, String::new());
        self.build_options.pending = Some(Pending {
            profile,
            format,
            response,
        });
        self.build_options.open = false;
        self.menu = None;
        self.rebuild(editor)
    }

    /// Poll the shared dialog future once per frame; exporting stays in build.rs.
    pub(crate) fn poll_build_options(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let answer = self
            .build_options
            .pending
            .as_mut()
            .and_then(|pending| pending.response.try_take());
        let Some(answer) = answer else {
            return Ok(false);
        };
        let pending = self
            .build_options
            .pending
            .take()
            .ok_or("Build output prompt lost its request")?;
        self.build_options.open = true;
        if let Some(destination) = answer {
            let result = (|| {
                if !self
                    .build_profiles()
                    .iter()
                    .any(|name| name == &pending.profile)
                {
                    return Err(
                        "The selected build profile was removed while choosing its output".into(),
                    );
                }
                let destination = destination.trim();
                let destination = (!destination.is_empty()).then(|| PathBuf::from(destination));
                match pending.format {
                    Some(format) => {
                        self.package_profile(editor, &pending.profile, destination, format)
                    }
                    None => self.build_profile(editor, &pending.profile, destination),
                }
            })();
            match result {
                Ok(()) => self.build_options.open = false,
                Err(error) => self.alert_error(editor, &*error),
            }
        }
        self.rebuild(editor)?;
        Ok(true)
    }

    /// Root modal composition uses the same scope, backdrop and bounded body as
    /// kit dialogs. Its profile field is the shared typed searchable Select.
    pub(crate) fn build_options_markup(&mut self, editor: &Editor, out: &mut String) {
        if !self.build_options_open() {
            return;
        }
        let width = ara_ui::popover::MAX_WIDTH.min((self.ui.size[0] - 16.).max(1.));
        let height = (self.ui.size[1] - 16.).max(1.);
        let names = self.build_profiles();
        let selected = self.selected_build_profile(editor);
        let current = selected
            .as_ref()
            .map_or(Value::Null, |profile| json!(profile));
        let package = self.build_options.package;
        let title = if package { "Package" } else { "Build options" };
        let confirm = if package {
            "package-options-build"
        } else {
            "build-options-build"
        };
        out.push_str(&format!("<modal id='{ROOT}' class='kit-dialog-layer' label='{title}' default-action='{confirm}'><panel id='build-options-backdrop' class='kit-dialog-backdrop' /><panel id='build-options-body' class='kit-dialog' style='width:{width}px;max-width:100%;max-height:{height}px;min-height:0px'><text id='build-options-title' class='kit-dialog-title' style='flex-shrink:0' text='{title}' /><scroll id='build-options-scroll' class='kit-dialog-body' style='min-height:0px;flex-basis:auto;flex-shrink:1;gap:8px'><panel id='build-options-profile' style='min-width:0px;flex-shrink:0;gap:4px'><text id='build-profile-label' text='Build profile' />"));
        self.choice_button(
            CONTROL,
            "Build profile",
            choices::Select {
                destination: choices::Destination::Setting(PROFILE_SETTING.into()),
                options: names
                    .iter()
                    .map(|name| (json!(name), name.clone()))
                    .collect(),
                current,
            },
            out,
        );
        out.push_str("</panel>");
        if package {
            out.push_str("<panel id='package-format-field' style='min-width:0px;flex-shrink:0;gap:4px'><text id='package-format-label' text='Package format' />");
            self.choice_button(
                "package-format-select",
                "Package format",
                choices::Select {
                    destination: choices::Destination::PackageFormat,
                    options: package_formats()
                        .into_iter()
                        .map(|(format, label)| (json!(format), label.into()))
                        .collect(),
                    current: json!(
                        self.build_options
                            .format
                            .unwrap_or(PackageFormat::WindowsPortable)
                    ),
                },
                out,
            );
            out.push_str("</panel><text id='package-format-help' class='caption' style='white-space:normal;flex-shrink:0' text='The format must match the profile platform. Installers and AppImages require their pinned tooling; Android distribution requires release signing. Ara checks these before starting.' />");
        }
        if let Err(error) = &selected {
            out.push_str(&format!("<text id='build-profile-problem' class='caption' style='white-space:normal;flex-shrink:0' text='{}' />", escape(&error.to_string())));
        }
        if names.is_empty() {
            self.bind_command(
                "build-profile-create",
                "build.profile.create",
                commands::Target::None,
            );
            out.push_str("<button id='build-profile-create' class='kit-dialog-button ghost' text='Create default profile' action='Dispatch' />");
        }
        out.push_str("<text id='build-options-output-note' class='caption' style='white-space:normal;flex-shrink:0' text='Build creates a new output folder for this profile. Choose output to provide a new destination instead.' />");
        if let Some(game) = &self.last_build {
            out.push_str(&format!("<text id='package-result-player-title' class='caption' style='flex-shrink:0' text='Last completed player folder' /><text id='package-result-player-path' class='mono caption' style='white-space:normal;flex-shrink:0' text='{}' />",escape(&game.to_string_lossy())));
        }
        if !self.last_packages.is_empty() {
            out.push_str("<text id='package-result-title' class='caption' text='Last completed package files' style='flex-shrink:0' />");
            for index in 0..self.last_packages.len() {
                let artifact = &self.last_packages[index];
                let path = artifact.path.to_string_lossy().into_owned();
                let bytes = artifact.bytes;
                let id = format!("package-result-{:016x}", ara_ui::source::stable_id(&path));
                self.bind_command(
                    &id,
                    "build.package.reveal",
                    commands::Target::Text(path.clone()),
                );
                out.push_str(&format!("<text id='{id}-path' class='mono caption' style='white-space:normal;flex-shrink:0' text='{}' /><button id='{id}' class='kit-dialog-button ghost' text='Reveal ({} bytes)' action='Dispatch' style='flex-shrink:0' />",escape(&path),bytes));
            }
        }
        out.push_str("</scroll><panel id='build-options-actions' class='kit-dialog-actions' style='flex-wrap:wrap;flex-shrink:0;min-width:0px'>");
        for (id, command, label, primary) in [
            ("build-options-close", "build.options.close", "Close", false),
            (
                "build-options-destination",
                "build.destination",
                "Choose output…",
                false,
            ),
            (
                confirm,
                if package {
                    "build.package.confirm"
                } else {
                    "build.current"
                },
                if package { "Package" } else { "Build" },
                true,
            ),
        ] {
            self.bind_command(id, command, commands::Target::None);
            let disabled = command != "build.options.close" && selected.is_err();
            out.push_str(&format!("<button id='{id}' class='kit-dialog-button{}' text='{label}' action='Dispatch'{} />", if primary {" kit-dialog-accept"} else {""}, if disabled {" disabled='true' description='Choose an available build profile first'"} else {""}));
        }
        out.push_str("</panel></panel></modal>");
    }

    /// Called after native Select/stacked shared dialogs have had first refusal.
    /// Queue the registry Close action so pointer dismissal uses normal dispatch.
    pub(crate) fn build_options_event(&mut self, event: &InputEvent) -> ara_ui::Result<bool> {
        if !self.build_options_open() || self.choice_open() || self.confirmations.is_open() {
            return Ok(false);
        }
        let close = matches!(event, InputEvent::Cancel)
            || matches!(event, InputEvent::PointerDown(point)
            if self.ui.find("build-options-body").is_some_and(|index| !self.ui.nodes[index].rect.contains(point[0],point[1])));
        if !close {
            return Ok(false);
        }
        if let Some(index) = self.ui.find("build-options-close") {
            self.ui.activate(index)?;
        }
        Ok(true)
    }
}

fn package_formats() -> [(PackageFormat, &'static str); 6] {
    [
        (PackageFormat::Folder, "Plain folder"),
        (PackageFormat::WindowsPortable, "Windows portable ZIP"),
        (PackageFormat::WindowsInstaller, "Windows installer + ZIP"),
        (PackageFormat::LinuxAppimage, "Linux AppImage"),
        (PackageFormat::LinuxTar, "Linux tar.gz"),
        (PackageFormat::AndroidPlay, "Android APK + AAB"),
    ]
}

fn selected_profile<'a>(names: &'a [String], selected: &str) -> ara_ui::Result<&'a str> {
    if !selected.is_empty() {
        return names
            .iter()
            .find(|name| name.as_str() == selected)
            .map(String::as_str)
            .ok_or_else(|| {
                format!("Build profile '{selected}' is unavailable. Choose an indexed profile.")
                    .into()
            });
    }
    names
        .iter()
        .find(|name| name.as_str() == build::DEFAULT_PROFILE)
        .or_else(|| names.first())
        .map(String::as_str)
        .ok_or_else(|| {
            "Create a build profile first, then wait for the project index to refresh".into()
        })
}

#[cfg(test)]
mod tests;
