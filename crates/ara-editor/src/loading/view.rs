//! What the loading window shows, as markup from a [`Status`]: pure, so every state
//! renders and tests without a window or an import pipeline.

/// Markup-escape for single-quoted attributes.
fn escape(text: &str) -> String {
    ara_ui::source::escape(text, b'\'')
}

/// One frame of the loading window.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Status {
    /// Project display name.
    pub project: String,
    /// Project folder, as shown under the name.
    pub folder: String,
    /// Whole seconds since loading started.
    pub elapsed: u64,
    /// Import counts; `None` until the pipeline first reports.
    pub imports: Option<Imports>,
    /// The source that finished most recently (project path).
    pub latest: Option<String>,
    /// "Open now" is offered: loading has run for a few seconds.
    pub skip: bool,
    /// Every import settled and the editor is opening.
    pub opening: bool,
    /// Where Preparing's indeterminate bar is, 0..1 (it sweeps while the pipeline
    /// has not reported); 0 in other stages, so their markup only changes with them.
    pub sweep: f32,
    /// Ara Light.
    pub light: bool,
}

/// Where the project's imports stand ([`ara_play_host::ImportProgress`], by path).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Imports {
    /// Sources with a current import.
    pub done: usize,
    /// Registered sources.
    pub total: usize,
    /// Sources still importing.
    pub pending: usize,
    /// Failed sources: project path and the import's error, sorted by path.
    pub failed: Vec<(String, String)>,
}

/// Width of Preparing's sweeping segment, in percent of the bar.
const SWEEP: f32 = 30.;

/// The stage a status is in; it decides the title, the bar and the footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Waiting for the pipeline's first report.
    Preparing,
    /// Sources are importing.
    Importing,
    /// Imports settled with failures: the window waits for "Open project".
    Review,
    /// Imports settled: the editor is opening.
    Opening,
}

impl Status {
    /// The stage this status is in.
    pub fn stage(&self) -> Stage {
        match &self.imports {
            _ if self.opening => Stage::Opening,
            None => Stage::Preparing,
            Some(i) if i.pending > 0 => Stage::Importing,
            Some(i) if !i.failed.is_empty() => Stage::Review,
            Some(_) => Stage::Opening,
        }
    }
    /// The footer's button: its id and label, if it shows one.
    pub fn button(&self) -> Option<(&'static str, &'static str)> {
        match self.stage() {
            Stage::Review => Some(("loading-open", "Open project")),
            Stage::Preparing | Stage::Importing if self.skip => Some(("loading-open", "Open now")),
            _ => None,
        }
    }
    /// Logical window height for this status: failures add rows (four at most).
    pub fn height(&self) -> f32 {
        let failed = self.imports.as_ref().map_or(0, |i| i.failed.len());
        if failed == 0 {
            BASE_HEIGHT
        } else {
            BASE_HEIGHT
                + 24.
                + failed.min(MAX_FAILURES) as f32 * 50.
                + if failed > MAX_FAILURES { 22. } else { 0. }
        }
    }
}

/// Logical window width and the height without failures.
pub(crate) const WIDTH: f32 = 520.;
pub(crate) const BASE_HEIGHT: f32 = 268.;
/// Failed imports listed by name; more are counted.
const MAX_FAILURES: usize = 4;

/// The window's markup for `status`.
pub(crate) fn markup(status: &Status) -> String {
    let stage = status.stage();
    let mut out = String::from("<ui id='loading'><panel id='loading-root' class='root'>");
    // The Hub's brand row: wordmark and a version tag.
    out.push_str(&format!(
        "<panel id='loading-head' class='head'><image id='loading-wordmark' class='wordmark' src='{}' /><panel id='loading-version' class='tag'><text id='loading-version-text' text='{}' /></panel></panel>",
        if status.light { "loading-wordmark-light" } else { "loading-wordmark" },
        escape(env!("CARGO_PKG_VERSION"))
    ));
    out.push_str(&format!(
        "<panel id='loading-project' class='project'><text id='loading-name' class='name' text='{}' /><text id='loading-folder' class='caption' text='{}' description='{}' /></panel>",
        escape(&status.project),
        escape(&middle(&status.folder, 64)),
        escape(&status.folder)
    ));
    let (title, count, fraction, detail) = match (stage, &status.imports) {
        (Stage::Opening, None) => (
            format!("Opening {}", status.project),
            String::new(),
            0.,
            "Assets import in the editor".to_owned(),
        ),
        (Stage::Preparing, _) | (_, None) => (
            "Preparing imports…".to_owned(),
            String::new(),
            0.,
            "Checking which sources need importing".to_owned(),
        ),
        (Stage::Importing, Some(i)) => (
            "Importing assets".to_owned(),
            format!("{} of {}", i.done + i.failed.len(), i.total),
            (i.done + i.failed.len()) as f32 / i.total.max(1) as f32,
            status.latest.as_ref().map_or_else(
                || "Starting the import workers".to_owned(),
                |path| format!("Imported {path}"),
            ),
        ),
        (Stage::Review, Some(i)) => (
            format!("Imported {} of {}", i.done, i.total),
            format!("{} failed", i.failed.len()),
            1.,
            "Failed assets show placeholders until their sources are fixed".to_owned(),
        ),
        // Opened early ("Open now"): the count stays honest; imports carry on.
        (Stage::Opening, Some(i)) if i.pending > 0 => (
            format!("Opening {}", status.project),
            format!("{} of {} imported", i.done + i.failed.len(), i.total),
            (i.done + i.failed.len()) as f32 / i.total.max(1) as f32,
            "The rest import in the editor, appearing as they finish".to_owned(),
        ),
        (Stage::Opening, Some(i)) => (
            format!("Opening {}", status.project),
            if i.total == 0 {
                String::new()
            } else {
                format!("{} assets", i.total)
            },
            1.,
            "Building the scene and compiling shaders".to_owned(),
        ),
    };
    let review = stage == Stage::Review;
    out.push_str(&format!(
        "<panel id='loading-stage' class='stage'><panel id='loading-stage-row' class='line'><text id='loading-title' class='title' text='{}' /><panel id='loading-stage-gap' class='fill' /><text id='loading-count' class='{}' text='{}' /></panel>",
        escape(&title),
        if review { "count-warning" } else { "count" },
        escape(&count)
    ));
    if stage == Stage::Preparing {
        // No counts yet: a segment sweeps back and forth instead of an empty bar.
        let at = (1. - (2. * status.sweep.clamp(0., 1.) - 1.).abs()) * (100. - SWEEP);
        out.push_str(&format!(
            "<panel id='loading-bar' class='bar' style='flex-direction:row'><panel id='loading-bar-gap' style='width:{at:.1}%;flex-shrink:0' /><panel id='loading-bar-fill' class='bar-fill' style='width:{SWEEP}%;flex-shrink:0' /></panel>"
        ));
    } else {
        out.push_str(&format!(
            "<panel id='loading-bar' class='bar'><panel id='loading-bar-fill' class='{}' style='width:{:.1}%' /></panel>",
            if review { "bar-fill bar-warning" } else { "bar-fill" },
            (fraction * 100.).clamp(0., 100.)
        ));
    }
    out.push_str(&format!(
        "<panel id='loading-detail-row' class='line'><text id='loading-detail' class='caption detail' text='{}' /><text id='loading-elapsed' class='caption elapsed' text='{}' /></panel></panel>",
        escape(&detail),
        clock(status.elapsed)
    ));
    if let Some(imports) = status.imports.as_ref().filter(|i| !i.failed.is_empty()) {
        out.push_str("<panel id='loading-failures' class='failures'>");
        for (n, (path, error)) in imports.failed.iter().take(MAX_FAILURES).enumerate() {
            let first = error.lines().next().unwrap_or_default();
            out.push_str(&format!(
                "<panel id='loading-failure-{n}' class='failure' description='{}'><image id='loading-failure-{n}-icon' class='icon warning' src='icon-triangle-alert' tint='true' /><panel id='loading-failure-{n}-text' class='failure-text'><text id='loading-failure-{n}-path' class='fg' text='{}' /><text id='loading-failure-{n}-error' class='caption' text='{}' /></panel></panel>",
                escape(error),
                escape(path),
                escape(first)
            ));
        }
        if imports.failed.len() > MAX_FAILURES {
            out.push_str(&format!(
                "<text id='loading-failures-more' class='caption more' text='{} more in Problems' />",
                imports.failed.len() - MAX_FAILURES
            ));
        }
        out.push_str("</panel>");
    }
    out.push_str(
        "<panel id='loading-spacer' class='fill' /><panel id='loading-foot' class='foot'>",
    );
    let hint = match stage {
        Stage::Preparing | Stage::Importing => "The editor opens when every import has finished.",
        Stage::Review => "Problems lists each failure with its fixes.",
        Stage::Opening => "The editor window appears in a moment.",
    };
    out.push_str(&format!(
        "<text id='loading-hint' class='caption' text='{}' /><panel id='loading-foot-gap' class='fill' />",
        escape(hint)
    ));
    if let Some((id, label)) = status.button() {
        out.push_str(&format!(
            "<button id='{id}' class='{}' label='{label}' description='{}' action='Dispatch'><text id='{id}-label' text='{label}' /></button>",
            if review { "primary" } else { "secondary" },
            if review {
                "Open the editor; failed assets keep placeholders"
            } else {
                "Open the editor now; imports keep running and models appear as they finish"
            }
        ));
    }
    out.push_str("</panel></panel></ui>");
    out
}

/// `m:ss` (or `h:mm:ss`).
fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `text` (a folder path) shortened to about `max` characters: its drive and first
/// folder, "…", then as many of its last folders as fit. A path whose last folder is
/// itself too long is elided by characters.
fn middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let separator = if text.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = text.split(separator).collect();
    if parts.len() > 3 {
        let head = parts[..2].join(&separator.to_string());
        let mut tail: Vec<&str> = Vec::new();
        for part in parts[2..].iter().rev() {
            let length = head.chars().count()
                + 3
                + tail.iter().map(|p| p.chars().count() + 1).sum::<usize>()
                + part.chars().count();
            if length > max {
                break;
            }
            tail.insert(0, part);
        }
        if !tail.is_empty() {
            let tail = tail.join(&separator.to_string());
            return format!("{head}{separator}…{separator}{tail}");
        }
    }
    let head = max / 3;
    let tail = max - head - 1;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!("{start}…{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(imports: Option<Imports>) -> Status {
        Status {
            project: "My Outpost".into(),
            folder: r"C:\Users\someone\Documents\Ara Projects\My Outpost".into(),
            elapsed: 42,
            imports,
            ..Default::default()
        }
    }

    #[test]
    fn stages_follow_the_imports() {
        assert_eq!(status(None).stage(), Stage::Preparing);
        let importing = Imports {
            done: 36,
            total: 115,
            pending: 78,
            failed: vec![("assets/a.gltf".into(), "bad".into())],
        };
        let s = status(Some(importing.clone()));
        assert_eq!(s.stage(), Stage::Importing);
        assert!(s.button().is_none(), "no escape in the first seconds");
        assert_eq!(
            Status {
                skip: true,
                ..s.clone()
            }
            .button(),
            Some(("loading-open", "Open now"))
        );
        let review = status(Some(Imports {
            pending: 0,
            done: 114,
            ..importing.clone()
        }));
        assert_eq!(review.stage(), Stage::Review);
        assert_eq!(review.button(), Some(("loading-open", "Open project")));
        assert!(review.height() > BASE_HEIGHT);
        let done = status(Some(Imports {
            done: 115,
            total: 115,
            ..Default::default()
        }));
        assert_eq!(done.stage(), Stage::Opening);
        assert_eq!(done.button(), None);
    }

    #[test]
    fn preparing_sweeps_a_segment_instead_of_showing_an_empty_bar() {
        let at = |sweep: f32| {
            let markup = markup(&Status {
                sweep,
                ..status(None)
            });
            assert!(markup.contains("text='Preparing imports…'"));
            assert!(markup.contains("id='loading-bar-fill' class='bar-fill' style='width:30%"));
            markup
        };
        assert_ne!(at(0.1), at(0.4), "the segment moves");
        assert!(at(0.5).contains("id='loading-bar-gap' style='width:70.0%"));
    }

    #[test]
    fn markup_counts_settled_sources_and_lists_failures() {
        let s = Status {
            latest: Some("assets/kit/apron.gltf".into()),
            ..status(Some(Imports {
                done: 36,
                total: 115,
                pending: 78,
                failed: vec![("assets/a.gltf".into(), "ARA4310 bad\nmore".into())],
            }))
        };
        let markup = markup(&s);
        assert!(
            markup.contains("text='37 of 115'"),
            "done and failed both count"
        );
        assert!(markup.contains("Imported assets/kit/apron.gltf"));
        assert!(markup.contains("text='0:42'"));
        assert!(
            markup.contains("text='ARA4310 bad'"),
            "the error's first line"
        );
        assert!(ara_ui::Ui::editor(&markup, super::super::CSS, [WIDTH, s.height()]).is_ok());
    }

    #[test]
    fn long_folders_keep_their_drive_and_last_folders() {
        assert_eq!(middle(r"C:\a\b", 64), r"C:\a\b");
        assert_eq!(
            middle(
                r"C:\Users\someone\AppData\Local\Temp\scratch-1\projects\My Outpost",
                48
            ),
            r"C:\Users\…\Temp\scratch-1\projects\My Outpost"
        );
        let long = middle(&format!(r"C:\{}", "x".repeat(80)), 40);
        assert_eq!(long.chars().count(), 40);
        assert!(long.starts_with(r"C:\x") && long.contains('…'));
        assert_eq!(clock(3725), "1:02:05");
    }
}
