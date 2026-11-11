//! The import gate. A project opens with its sources importing in the background; until
//! they settle the studio shows a modal card over its window (the last source to finish,
//! N of M, the elapsed time, and every failed import with its error) and keeps its native
//! views (Scene, Game, graph canvases) hidden, so a half-imported scene is never presented
//! as finished. Imports that settle within [`SHOW_AFTER`] never show the card, and the gate
//! opens by itself when they settle clean. After a few seconds "Open now" lets a person in
//! early (imports carry on); failures hold the card on a review until "Open project".
//!
//! The host loop owns a [`Gate`], steps it every pass and mirrors what it says into
//! `Local::import_gate` for the view ([`overlay`]).
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ara_kit::icons;
use ara_kit::ui::*;
use ara_play_host::{ImportProgress, PublishedAssets};
use stillkit::el::{Align, Handled, KeyCode, Overlay, col, el, row, text};

use super::Studio;
use crate::Editor;
use crate::import_gate::{Imports, imports as imports_of, source_paths};

/// Imports that settle sooner never show the card.
const SHOW_AFTER: Duration = Duration::from_millis(400);
/// When "Open now" appears.
const SKIP_AFTER: Duration = Duration::from_secs(3);
/// A pipeline that has not reported by then is not coming (no import worker): open.
const GIVE_UP: Duration = Duration::from_secs(12);
/// Failed imports listed by name; more are counted.
const MAX_FAILURES: usize = 4;

/// What the card shows for one pass of the gate.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Standing {
    /// Project display name.
    pub project: String,
    /// Project folder.
    pub folder: String,
    /// Whole seconds since the gate started.
    pub elapsed: u64,
    /// The card is on screen: the imports have outlasted [`SHOW_AFTER`].
    pub shown: bool,
    /// Import counts; `None` until the pipeline first reports.
    pub imports: Option<Imports>,
    /// The source that finished most recently (project path).
    pub latest: Option<String>,
    /// "Open now" is offered: the wait has passed [`SKIP_AFTER`].
    pub skip: bool,
}

/// The stage a standing is in; it decides the title, the bar and the footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Waiting for the pipeline's first report.
    Preparing,
    /// Sources are importing.
    Importing,
    /// Imports settled with failures: the card waits for "Open project".
    Review,
}

impl Standing {
    /// The stage this standing is in (a clean settle is no standing: the gate opens).
    pub(crate) fn stage(&self) -> Stage {
        match &self.imports {
            None => Stage::Preparing,
            Some(i) if i.pending > 0 => Stage::Importing,
            Some(_) => Stage::Review,
        }
    }
}

/// Follows the import feed: the latest progress, the source that finished last and asset
/// ids as project paths.
struct Tracker {
    reader: Option<PublishedAssets>,
    revision: u64,
    progress: Option<Arc<ImportProgress>>,
    /// The source that finished most recently.
    latest: Option<String>,
    /// Asset id → source path (from the import documents' paths).
    paths: BTreeMap<String, String>,
}

impl Tracker {
    fn new(editor: &Editor) -> Self {
        let paths = source_paths(editor.host.store().workspace());
        Self {
            reader: PublishedAssets::for_project(editor.host.root()).ok(),
            revision: 0,
            progress: None,
            latest: None,
            paths,
        }
    }

    /// Reads new progress from the feed, if it has changed.
    fn poll(&mut self) {
        let Some(reader) = &self.reader else {
            return;
        };
        let revision = reader.progress_revision();
        if revision == self.revision {
            return;
        }
        let Some(progress) = reader.progress() else {
            return;
        };
        self.revision = revision;
        self.observe(progress);
    }

    /// Takes `progress` as the latest, noting a source that left `pending` without failing.
    fn observe(&mut self, progress: Arc<ImportProgress>) {
        if let Some(before) = &self.progress {
            let finished: BTreeSet<&String> = before
                .pending
                .difference(&progress.pending)
                .filter(|id| !progress.failed.contains_key(*id))
                .collect();
            if let Some(id) = finished.last() {
                self.latest = Some(self.path(id));
            }
        }
        self.progress = Some(progress);
    }

    fn path(&self, id: &str) -> String {
        self.paths.get(id).cloned().unwrap_or_else(|| id.to_owned())
    }

    /// Every import settled with no failures.
    fn settled_clean(&self) -> bool {
        self.progress
            .as_ref()
            .is_some_and(|p| p.pending.is_empty() && p.failed.is_empty())
    }

    fn imports(&self) -> Option<Imports> {
        Some(imports_of(self.progress.as_ref()?, &self.paths))
    }
}

/// The host loop's end of the gate: reads the import feed and says what the card shows.
pub(super) struct Gate {
    tracker: Tracker,
    started: Instant,
    project: String,
    folder: String,
    /// Nobody is at the window (a background or capture start): the gate opens once the
    /// imports settle, failures or not, so an agent's captures are never held for a person.
    unattended: bool,
    /// Who let themselves in before the imports settled.
    released: Option<&'static str>,
    /// The furthest stage logged, so each is said once.
    logged: Logged,
    /// Log lines not yet taken ([`Gate::take_log`]).
    lines: Vec<String>,
}

/// The stages the gate logs, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Logged {
    Nothing,
    Importing,
    Review,
    Opened,
}

impl Gate {
    /// A gate for the project `editor` has open, started now. `unattended` opens it by
    /// itself once the imports settle, whatever they came to.
    pub(super) fn new(editor: &Editor, unattended: bool) -> Self {
        Self {
            tracker: Tracker::new(editor),
            started: Instant::now(),
            project: project_name(editor),
            folder: dunce::simplified(editor.host.root()).display().to_string(),
            unattended,
            released: None,
            logged: Logged::Nothing,
            lines: Vec::new(),
        }
    }

    /// Opens the gate now (`by` says who: a person at the card, an agent over the protocol).
    pub(super) fn release(&mut self, by: &'static str) {
        self.released.get_or_insert(by);
    }

    /// The log lines since the last call: one per stage change (importing, review, opened),
    /// for the console, so a real import can be checked without a screenshot.
    pub(super) fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }

    /// Reads the feed. `Some` is what the card shows and the gate holds; `None` means open:
    /// every import settled clean, a person or an agent let themselves in, the gate is
    /// unattended and the imports settled, or the pipeline never reported (no import worker).
    pub(super) fn step(&mut self) -> Option<Standing> {
        self.tracker.poll();
        let standing = self.standing();
        self.log(standing.as_ref());
        standing
    }

    /// Why the gate is open, if it is.
    fn opened(&self) -> Option<&'static str> {
        let imports = self.tracker.imports();
        if let Some(by) = self.released {
            Some(by)
        } else if self.tracker.settled_clean() {
            Some("all imported")
        } else if self.unattended && imports.as_ref().is_some_and(Imports::settled) {
            Some("no one at the window")
        } else if imports.is_none() && self.started.elapsed() >= GIVE_UP {
            Some("no report from the import pipeline")
        } else {
            None
        }
    }

    /// Says each stage once: importing, review (with the failures and their errors), opened.
    fn log(&mut self, standing: Option<&Standing>) {
        match (standing, self.opened()) {
            (Some(standing), _) => match standing.stage() {
                Stage::Importing if self.logged < Logged::Importing => {
                    let i = standing.imports.as_ref().map(|i| (i.done, i.total));
                    let (done, total) = i.unwrap_or_default();
                    self.lines
                        .push(format!("imports: importing {done} of {total}"));
                    self.logged = Logged::Importing;
                }
                Stage::Review if self.logged < Logged::Review => {
                    let failed = standing.imports.as_ref().map_or(&[][..], |i| &i.failed);
                    self.lines.push(review_line(failed));
                    self.logged = Logged::Review;
                }
                _ => {}
            },
            (None, why) if self.logged < Logged::Opened => {
                let failed = self.tracker.imports().map(|i| i.failed).unwrap_or_default();
                // A gate that opens past its review (unattended, or released at once) still
                // says what failed.
                if !failed.is_empty() && self.logged < Logged::Review {
                    self.lines.push(review_line(&failed));
                }
                self.lines.push(format!(
                    "imports: opened ({}), {} failed",
                    why.unwrap_or("open"),
                    failed.len()
                ));
                self.logged = Logged::Opened;
            }
            _ => {}
        }
    }

    fn standing(&self) -> Option<Standing> {
        if self.opened().is_some() {
            return None;
        }
        let elapsed = self.started.elapsed();
        let imports = self.tracker.imports();
        Some(Standing {
            project: self.project.clone(),
            folder: self.folder.clone(),
            elapsed: elapsed.as_secs(),
            shown: elapsed >= SHOW_AFTER,
            imports,
            latest: self.tracker.latest.clone(),
            skip: elapsed >= SKIP_AFTER,
        })
    }

    /// Feeds `progress` as the pipeline's report, as a test's stand-in for the feed.
    #[cfg(test)]
    pub(super) fn observe(&mut self, progress: ImportProgress) {
        self.tracker.observe(Arc::new(progress));
    }

    /// A gate that started `ago` earlier, for the card's waiting states.
    #[cfg(test)]
    pub(super) fn started_ago(mut self, ago: Duration) -> Self {
        self.started = Instant::now() - ago;
        self
    }

    /// A gate reading no feed: its pipeline never reports on its own.
    #[cfg(test)]
    pub(super) fn without_feed(mut self) -> Self {
        self.tracker.reader = None;
        self
    }
}

/// The log line for a review: each failed source with the first line of its error.
fn review_line(failed: &[(String, String)]) -> String {
    let list: Vec<String> = failed
        .iter()
        .map(|(path, error)| format!("{path}: {}", error.lines().next().unwrap_or_default()))
        .collect();
    format!(
        "imports: review with {} failure{}: {}",
        failed.len(),
        if failed.len() == 1 { "" } else { "s" },
        list.join("; ")
    )
}

/// The project's name as people read it: `name` in `ara.project.json`, else its folder's.
fn project_name(editor: &Editor) -> String {
    editor
        .state
        .settings
        .get("project.name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let folder = editor.host.root().file_name()?;
            Some(folder.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Project".into())
}

/// Elapsed seconds as `m:ss`.
fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The card, while the gate holds and the wait has outlasted [`SHOW_AFTER`].
pub(super) fn overlay(s: &Studio) -> Option<Overlay<Studio>> {
    let standing = s.local.import_gate.as_ref().filter(|g| g.shown)?;
    if s.local.import_released {
        return None;
    }
    let l = &s.look;
    let c = &l.c;
    let stage = standing.stage();
    let open = |s: &mut Studio| s.local.import_released = true;
    let (title, detail, fraction) = match (&standing.imports, stage) {
        (None, _) => (
            "Preparing imports…".to_owned(),
            "Checking which sources need importing".to_owned(),
            None,
        ),
        (Some(i), Stage::Importing) => (
            format!("Importing {} of {}", i.done, i.total),
            match &standing.latest {
                Some(path) => format!("Last: {path}"),
                None => format!("{} sources left", i.pending),
            },
            Some(i.done as f32 / i.total.max(1) as f32),
        ),
        (Some(i), _) => (
            match i.failed.len() {
                1 => "1 import failed".to_owned(),
                n => format!("{n} imports failed"),
            },
            format!("{} of {} imported", i.done, i.total),
            Some(1.0),
        ),
    };
    let mut body = col().gap(10.0).child(
        col()
            .gap(1.0)
            .child(
                text(standing.project.clone())
                    .key("loading/project")
                    .text_size(13.0)
                    .weight(600)
                    .color(c.foreground.0)
                    .wrap(1),
            )
            .child(
                text(standing.folder.clone())
                    .text_size(11.0)
                    .color(c.muted_foreground.0)
                    .wrap(1),
            ),
    );
    let bar = match fraction {
        Some(fraction) => progress(
            l,
            fraction,
            if stage == Stage::Review {
                Status::Warning
            } else {
                Status::Info
            },
        )
        .grow(1.0),
        None => el().h(4.0).grow(1.0),
    };
    body = body.child(
        row()
            .gap(10.0)
            .align(Align::Center)
            .children((stage != Stage::Review).then(|| spinner(l, 14.0)))
            .child(bar)
            .child(
                // A fixed width: a clock that shrinks to fit clips to "1:…".
                text(clock(standing.elapsed))
                    .key("loading/elapsed")
                    .text_size(11.5)
                    .color(c.muted_foreground.0)
                    .w(40.0),
            ),
    );
    body = body.child(
        text(detail)
            .key("loading/detail")
            .text_size(12.0)
            .color(c.muted_foreground.0)
            .wrap(1),
    );
    if let Some(imports) = &standing.imports {
        let shown = imports.failed.iter().take(MAX_FAILURES);
        let mut list = col().key("loading/failed").gap(6.0);
        for (path, error) in shown {
            list = list.child(
                col()
                    .gap(1.0)
                    .pad(8.0)
                    .radius(l.radius.base)
                    .fill(c.destructive.0.with_alpha(0.1))
                    .child(
                        text(path.clone())
                            .text_size(12.0)
                            .weight(600)
                            .color(c.foreground.0)
                            .wrap(1),
                    )
                    .child(
                        text(error.clone())
                            .text_size(11.5)
                            .color(c.muted_foreground.0)
                            .wrap(3),
                    ),
            );
        }
        if imports.failed.len() > MAX_FAILURES {
            list = list.child(
                text(format!("and {} more", imports.failed.len() - MAX_FAILURES))
                    .text_size(11.5)
                    .color(c.muted_foreground.0),
            );
        }
        if !imports.failed.is_empty() {
            body = body.child(list);
        }
    }
    let mut buttons = Vec::new();
    match stage {
        Stage::Review => buttons.push(
            button(l, Tone::Primary, Size::Md, None, "Open project")
                .key("loading/open")
                .on_press(open),
        ),
        _ if standing.skip => buttons.push(
            button(l, Tone::Ghost, Size::Md, None, "Open now")
                .key("loading/open")
                .tooltip("Open the project while its imports carry on")
                .on_press(open),
        ),
        _ => {}
    }
    let status = (stage == Stage::Review).then_some((Status::Warning, icons::CIRCLE_ALERT));
    // Enter takes the card's button, as the old loading window's did: Open project on a
    // review, Open now once it is offered.
    let enter = stage == Stage::Review || standing.skip;
    let panel = dialog_panel(l, status, &title, body, buttons, open)
        .w(480.0)
        .on_key(move |s: &mut Studio, key, _| {
            if enter && key.0 == KeyCode::ENTER.0 {
                s.local.import_released = true;
                Handled::Yes
            } else {
                Handled::No
            }
        });
    // No dismiss on a press outside or Esc: only the card's own buttons let a person in.
    Some(stillkit::el::dialog(panel))
}
