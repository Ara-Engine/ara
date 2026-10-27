//! The setup window. [`View`] is everything it can show, built from the install
//! transaction's progress; [`paint`] draws a view into an offscreen bitmap (the
//! window blits it, tests save it as PNG); [`window`] runs the Win32 window.
mod canvas;
pub mod paint;
pub mod window;

use crate::flow::{Installed, Kind, Plan, Step, VERSION};
use crate::processes::Blocker;
use std::path::PathBuf;

/// What a button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Replace the existing installation.
    Replace,
    /// Leave everything unchanged and close.
    Cancel,
    /// Check the blocking programs again.
    Retry,
    /// Stop the Hub service gracefully.
    StopService,
    /// Put the full error text on the clipboard.
    CopyDetails,
    /// Close the window.
    Close,
    /// Start Ara Hub and close.
    Launch,
}

/// A footer button; the primary one is rightmost and answers Enter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Button {
    /// What it does.
    pub action: Action,
    /// Its text.
    pub label: &'static str,
    /// Filled with the accent colour.
    pub primary: bool,
}

/// Progress while setup works; nothing to click.
#[derive(Clone, Debug, PartialEq)]
pub struct Busy {
    /// `Installing Ara 0.0.1-alpha`.
    pub title: String,
    /// The destination, or what setup is checking.
    pub subtitle: String,
    /// The current step.
    pub label: String,
    /// The current file, or a step detail.
    pub detail: String,
    /// `245 MB of 612 MB`.
    pub amount: String,
    /// Overall progress, 0..=1.
    pub fraction: f32,
}

/// One state of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum View {
    /// Verifying, extracting, swapping or registering.
    Busy(Busy),
    /// A same-kind installation exists.
    Confirm {
        /// `Replace Ara 0.0.1-alpha?`
        title: String,
        /// Installed versus this setup's version.
        subtitle: String,
        /// `Ara 0.0.1-alpha is already installed at`
        lead: String,
        /// The existing installation.
        destination: String,
    },
    /// Programs block the destination.
    Blocked {
        /// `Close Ara to continue`.
        title: String,
        /// What blocks the folder.
        subtitle: String,
        /// `(name, "ara-hub.exe · PID 12")` per program.
        programs: Vec<(String, String)>,
        /// The Hub service is among them and can be stopped gracefully.
        can_stop: bool,
    },
    /// Setup failed; it never closes by itself.
    Error {
        /// `Couldn't install Ara Hub`.
        title: String,
        /// `ARA-EXP-INSTALL-004`, when the error has one.
        code: Option<String>,
        /// First paragraph of the error.
        message: String,
        /// Everything, for Copy details.
        details: String,
        /// Copy details was used.
        copied: bool,
    },
    /// Installed.
    Done {
        /// `Ara 0.0.1-alpha installed`.
        title: String,
        /// Installation directory.
        destination: String,
        /// What to do next.
        note: String,
        /// Something to know (leftover replaced files).
        warning: Option<String>,
        /// Ara Hub to start from the primary button.
        launch: Option<PathBuf>,
    },
}

const fn button(action: Action, label: &'static str, primary: bool) -> Button {
    Button {
        action,
        label,
        primary,
    }
}

impl View {
    /// Before the payload is verified.
    pub fn preparing() -> View {
        View::Busy(Busy {
            title: "Ara Setup".into(),
            subtitle: "Checking the installer".into(),
            label: "Preparing".into(),
            detail: String::new(),
            amount: String::new(),
            fraction: 0.0,
        })
    }

    /// Progress of `step`; `plan` is known after verification.
    pub fn progress(plan: Option<&Plan>, step: Step<'_>) -> View {
        let (title, subtitle) = match plan {
            Some(plan) => (
                format!("Installing {}", plan.kind.product()),
                plan.destination.display().to_string(),
            ),
            None => ("Ara Setup".into(), "Checking the installer".into()),
        };
        let part = |done: u64, total: u64| done as f32 / total.max(1) as f32;
        let (label, detail, amount, fraction) = match step {
            Step::Verify { done, total } => (
                "Verifying the installer".into(),
                String::new(),
                amount(done, total),
                0.06 * part(done, total),
            ),
            Step::Extract { file, done, total } => (
                "Extracting files".into(),
                file.replace('/', "\\"),
                amount(done, total),
                0.06 + 0.84 * part(done, total),
            ),
            Step::StopService => (
                "Stopping the Ara Hub service".into(),
                "It starts again when it is needed".into(),
                String::new(),
                0.06,
            ),
            Step::Swap => (
                "Moving files into place".into(),
                String::new(),
                String::new(),
                0.91,
            ),
            Step::Cleanup => (
                "Removing the replaced files".into(),
                String::new(),
                String::new(),
                0.93,
            ),
            Step::Register => match plan.map(|plan| plan.kind) {
                Some(Kind::Hub) => (
                    "Setting up Ara Hub".into(),
                    "Shortcuts and the Hub service".into(),
                    String::new(),
                    0.95,
                ),
                _ => (
                    "Registering with Ara Hub".into(),
                    "Shortcuts, PATH and a prerequisites check".into(),
                    String::new(),
                    0.95,
                ),
            },
        };
        View::Busy(Busy {
            title,
            subtitle,
            label,
            detail,
            amount,
            fraction,
        })
    }

    /// Asks whether to replace the `installed` version at the plan's destination.
    pub fn confirm(plan: &Plan, installed: &str) -> View {
        let named = match plan.kind {
            Kind::Engine => format!("Ara {installed}"),
            Kind::Hub => format!("Ara Hub {installed}"),
        };
        View::Confirm {
            title: if installed == VERSION {
                format!("Replace {}?", plan.kind.product())
            } else {
                format!("Update {}?", plan.kind.product())
            },
            subtitle: if installed == VERSION {
                "This version is already installed".into()
            } else {
                format!("Installed: {installed} · This setup: {VERSION}")
            },
            lead: format!("{named} is already installed at"),
            destination: plan.destination.display().to_string(),
        }
    }

    /// Programs (possibly none detected) keep the destination in use.
    pub fn blocked(blockers: &[Blocker]) -> View {
        View::Blocked {
            title: "Close Ara to continue".into(),
            subtitle: if blockers.is_empty() {
                "A program is using files in the installation folder".into()
            } else {
                "These programs are running from the installation folder".into()
            },
            programs: blockers
                .iter()
                .map(|blocker| {
                    (
                        blocker.label(),
                        format!("{} · PID {}", blocker.exe, blocker.pid),
                    )
                })
                .collect(),
            can_stop: blockers.iter().any(|blocker| blocker.service),
        }
    }

    /// The transaction failed.
    pub fn failed(plan: Option<&Plan>, error: &anyhow::Error) -> View {
        let (code, summary) = crate::flow::summarize(error);
        let mut chars = summary.chars();
        let message = chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        let mut details = format!("Ara Setup {VERSION}\n");
        if let Some(plan) = plan {
            details.push_str(&format!(
                "Installing: {}\nDestination: {}\n",
                plan.kind.product(),
                plan.destination.display()
            ));
        }
        details.push_str(&format!("\n{error:#}\n"));
        View::Error {
            title: match plan {
                Some(plan) => format!("Couldn't install {}", plan.kind.product()),
                None => "Setup couldn't start".into(),
            },
            code,
            message,
            details,
            copied: false,
        }
    }

    /// Installed; `no_system_integration` keeps the launch on this installation.
    pub fn done(installed: &Installed, no_system_integration: bool) -> View {
        let next = match installed.plan.kind {
            Kind::Engine => "Open Ara Hub to create a project or try a sample.",
            Kind::Hub => "Open Ara Hub to install an engine version and create a project.",
        };
        View::Done {
            title: format!("{} installed", installed.plan.kind.product()),
            destination: installed.plan.destination.display().to_string(),
            note: if installed.replaced {
                format!("Replaced the previous installation. {next}")
            } else {
                next.to_owned()
            },
            warning: installed.leftover.as_ref().map(|leftover| {
                format!(
                    "Couldn't delete the replaced files at {}",
                    leftover.display()
                )
            }),
            launch: hub_executable(installed, no_system_integration),
        }
    }

    /// The footer buttons, primary last.
    pub fn buttons(&self) -> Vec<Button> {
        use Action::*;
        match self {
            View::Busy(_) => Vec::new(),
            View::Confirm { title, .. } => vec![
                button(Cancel, "Cancel", false),
                button(
                    Replace,
                    if title.starts_with("Update ") {
                        "Update now"
                    } else {
                        "Replace installation"
                    },
                    true,
                ),
            ],
            View::Blocked { can_stop: true, .. } => vec![
                button(Cancel, "Cancel", false),
                button(Retry, "Retry", false),
                button(StopService, "Stop Hub service", true),
            ],
            View::Blocked { .. } => vec![
                button(Cancel, "Cancel", false),
                button(Retry, "Retry", true),
            ],
            View::Error { copied, .. } => vec![
                button(
                    CopyDetails,
                    if *copied { "Copied" } else { "Copy details" },
                    false,
                ),
                button(Close, "Close", true),
            ],
            View::Done {
                launch: Some(_), ..
            } => vec![
                button(Close, "Close", false),
                button(Launch, "Launch Ara Hub", true),
            ],
            View::Done { .. } => vec![button(Close, "Close", true)],
        }
    }

    /// What Enter does.
    pub fn primary(&self) -> Option<Action> {
        self.buttons()
            .into_iter()
            .find(|button| button.primary)
            .map(|button| button.action)
    }

    /// What Esc and the window's Close do; `None` while setup is busy.
    pub fn dismiss(&self) -> Option<Action> {
        match self {
            View::Busy(_) => None,
            View::Confirm { .. } | View::Blocked { .. } => Some(Action::Cancel),
            View::Error { .. } | View::Done { .. } => Some(Action::Close),
        }
    }
}

/// `245 MB of 612 MB`.
fn amount(done: u64, total: u64) -> String {
    format!("{} of {}", size(done), size(total))
}

fn size(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

/// The Hub to start after installing: the shared standalone Hub that the engine
/// registration installs or reuses, else the one inside the installation.
fn hub_executable(installed: &Installed, no_system_integration: bool) -> Option<PathBuf> {
    let own = installed.plan.destination.join("ara-hub.exe");
    let shared = match installed.plan.kind {
        Kind::Engine if !no_system_integration => Kind::Hub
            .default_destination()
            .ok()
            .map(|hub| hub.join("ara-hub.exe")),
        _ => None,
    };
    shared
        .filter(|hub| hub.is_file())
        .or_else(|| own.is_file().then_some(own))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_monotonic_across_steps_and_sizes_read_well() {
        let plan = Plan {
            kind: Kind::Engine,
            destination: PathBuf::from(r"C:\Ara\0.0.1-alpha"),
        };
        let fraction = |step| match View::progress(Some(&plan), step) {
            View::Busy(busy) => busy.fraction,
            _ => -1.0,
        };
        let steps = [
            Step::Verify { done: 0, total: 10 },
            Step::Verify {
                done: 10,
                total: 10,
            },
            Step::Extract {
                file: "a",
                done: 5,
                total: 10,
            },
            Step::Extract {
                file: "b",
                done: 10,
                total: 10,
            },
            Step::Swap,
            Step::Cleanup,
            Step::Register,
        ];
        let fractions = steps.map(fraction);
        assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{fractions:?}");
        assert!(fractions[6] < 1.0);
        assert_eq!(size(612 * 1024 * 1024), "612 MB");
        assert_eq!(size(1_717_986_918), "1.6 GB");
        assert_eq!(size(512 * 1024), "0.5 MB");
    }

    #[test]
    fn keyboard_actions_follow_the_buttons() {
        let blocked = View::Blocked {
            title: String::new(),
            subtitle: String::new(),
            programs: Vec::new(),
            can_stop: true,
        };
        assert_eq!(blocked.primary(), Some(Action::StopService));
        assert_eq!(blocked.dismiss(), Some(Action::Cancel));
        assert_eq!(View::preparing().dismiss(), None);
        assert!(View::preparing().buttons().is_empty());
    }
}
