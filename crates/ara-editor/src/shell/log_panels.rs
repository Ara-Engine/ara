//! Console and Problems, both read from the project's diagnostics bus (`logs`, the
//! same stream `ara logs` and agents read):
//! - **Console** is the chronological log: severity toggles with counts, collapse by
//!   code, clear, search, time, origin and a source link per row; a selected row
//!   expands its details and fixes.
//! - **Problems** is what is wrong now: errors and warnings grouped by file or
//!   document, with one-click fixes and "Why", the catalog's explanation.
//!
//! Rows are two lines at every width (as in Unity's console): the message, then its
//! time, origin and source (Problems: code and source). Both panels live in narrow
//! docks by default, so nothing may sit over the message text.
use super::commands::Target;
use super::*;
use std::collections::BTreeMap;

/// Console and Problems filters: panel-local view state (neither navigation nor undo).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LogView {
    /// Console shows errors.
    pub errors: bool,
    /// Console shows warnings.
    pub warnings: bool,
    /// Console shows info messages.
    pub info: bool,
    /// Console merges rows with the same code into one.
    pub collapse: bool,
    /// Console shows only records after this bus cursor (Clear).
    pub since: u64,
    /// Problems shows errors.
    pub problem_errors: bool,
    /// Problems shows warnings.
    pub problem_warnings: bool,
}
impl Default for LogView {
    fn default() -> Self {
        Self {
            errors: true,
            warnings: true,
            info: true,
            collapse: false,
            since: 0,
            problem_errors: true,
            problem_warnings: true,
        }
    }
}

/// One bus record, as `logs` returns it.
struct Entry {
    seq: u64,
    count: u64,
    at: u64,
    origin: String,
    diagnostic: ara_diag::Diagnostic,
}

fn entries(editor: &mut Editor, since: u64, severity: &str) -> Vec<Entry> {
    let logs = editor
        .action(
            "logs",
            json!({"group": false, "limit": 500, "since": since, "severity": severity}),
        )
        .unwrap_or(Value::Null);
    logs["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(Entry {
                seq: e["seq"].as_u64()?,
                count: e["count"].as_u64().unwrap_or(1),
                at: e["at"].as_u64().unwrap_or(0),
                origin: e["origin"].as_str().unwrap_or("engine").to_owned(),
                diagnostic: serde_json::from_value(e["diagnostic"].clone()).ok()?,
            })
        })
        .collect()
}

fn severity_icon(severity: ara_diag::Severity) -> (&'static str, &'static str) {
    match severity {
        ara_diag::Severity::Error => ("circle-x", "destructive"),
        ara_diag::Severity::Warning => ("triangle-alert", "warning"),
        _ => ("info", "info"),
    }
}

/// A diagnostic's file as a project path (`scripts/Outpost/Spinner.cs`) when it lies
/// in the project (compilers report absolute paths); other files stay as they are.
fn project_file(editor: &Editor, file: &str) -> String {
    let file = file.replace('\\', "/");
    let root = editor.host.root().to_string_lossy().replace('\\', "/");
    let root = root.trim_start_matches("//?/").trim_end_matches('/');
    match file.get(..root.len()) {
        Some(prefix)
            if prefix.eq_ignore_ascii_case(root) && file[root.len()..].starts_with('/') =>
        {
            file[root.len() + 1..].to_owned()
        }
        _ => file,
    }
}

/// Where a diagnostic points, as a short label and the command that reveals it.
fn source(editor: &Editor, d: &ara_diag::Diagnostic) -> Option<(String, Binding)> {
    if let Some(record) = &d.location.record {
        let doc = d.location.doc.as_deref().unwrap_or(&editor.state.doc);
        let name = crate::world::resolve(editor.host.store(), doc)
            .ok()
            .and_then(|r| {
                r.get(record.as_str())
                    .and_then(|e| e.record.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| record.clone());
        let label = match &d.location.path {
            Some(path) => format!("{name} · {path}"),
            None => name,
        };
        return Some((label, Binding::Diagnostic(d.clone())));
    }
    let path = d
        .location
        .file
        .as_deref()
        .map(|file| project_file(editor, file))
        .or_else(|| {
            d.location.doc.as_deref().and_then(|doc| {
                editor
                    .host
                    .store()
                    .workspace()
                    .path_of(doc)
                    .map(str::to_owned)
            })
        })
        .or_else(|| {
            d.subjects.iter().find_map(|s| {
                let text = s.to_string();
                text.strip_prefix("asset:")
                    .map(|a| a.trim_start_matches('@').to_owned())
            })
        })?;
    let label = match d.location.line {
        Some(line) => format!("{path}:{line}"),
        None => path.clone(),
    };
    Some((label, Binding::Command("assets.show", Target::Asset(path))))
}

/// Local wall-clock `HH:MM:SS` of a Unix time in milliseconds.
fn clock(at_ms: u64) -> String {
    let minutes = (at_ms / 60_000) as i64 + local_offset_minutes();
    let seconds = (at_ms / 1000) % 60;
    let day = minutes.rem_euclid(24 * 60);
    format!("{:02}:{:02}:{seconds:02}", day / 60, day % 60)
}

/// The local time zone's offset from UTC, in minutes (0 off Windows).
fn local_offset_minutes() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        #[cfg(windows)]
        {
            // SAFETY: both calls only fill the returned SYSTEMTIME; no pointers escape.
            let (local, utc) = unsafe {
                (
                    windows::Win32::System::SystemInformation::GetLocalTime(),
                    windows::Win32::System::SystemInformation::GetSystemTime(),
                )
            };
            let minutes = |t: &windows::Win32::Foundation::SYSTEMTIME| {
                i64::from(t.wHour) * 60 + i64::from(t.wMinute)
            };
            let mut offset = minutes(&local) - minutes(&utc);
            if offset > 14 * 60 {
                offset -= 24 * 60;
            } else if offset < -14 * 60 {
                offset += 24 * 60;
            }
            offset
        }
        #[cfg(not(windows))]
        {
            0
        }
    })
}

impl Shell {
    pub(super) fn console_panel(&mut self, editor: &mut Editor, out: &mut String) {
        let view = self.log_view;
        let all = entries(editor, view.since, "info");
        let count = |s: ara_diag::Severity| {
            all.iter()
                .filter(|e| e.diagnostic.severity == s)
                .map(|e| e.count)
                .sum::<u64>()
        };
        out.push_str("<panel id='console-toolbar' class='toolbar'>");
        self.tool_icon(
            out,
            "console-clear",
            "console.clear",
            "trash",
            editor,
            false,
        );
        self.tool_icon(
            out,
            "console-collapse",
            "console.collapse",
            "layers",
            editor,
            view.collapse,
        );
        out.push_str("<panel id='console-sep' class='toolbar-sep' />");
        for (id, command, severity, on) in [
            (
                "console-errors",
                "console.errors",
                ara_diag::Severity::Error,
                view.errors,
            ),
            (
                "console-warnings",
                "console.warnings",
                ara_diag::Severity::Warning,
                view.warnings,
            ),
            (
                "console-info",
                "console.info",
                ara_diag::Severity::Info,
                view.info,
            ),
        ] {
            let (icon, tint) = severity_icon(severity);
            self.bind_command(id, command, Target::None);
            out.push_str(&format!("<button id='{id}' class='{} count-toggle' label='{}' description='Show or hide {}' action='Dispatch'>{}<text id='{id}-n' text='{}' /></button>",if on{"segment-on"}else{"ghost"},match severity{ara_diag::Severity::Error=>"Errors",ara_diag::Severity::Warning=>"Warnings",_=>"Messages"},match severity{ara_diag::Severity::Error=>"errors",ara_diag::Severity::Warning=>"warnings",_=>"info messages"},icons::icon_in(id,icon,if on{tint}else{"muted"}),count(severity)));
        }
        self.search(out, "console", editor);
        out.push_str("</panel>");
        let query = editor
            .state
            .searches
            .get("console")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let mut rows: Vec<&Entry> = all
            .iter()
            .filter(|e| match e.diagnostic.severity {
                ara_diag::Severity::Error => view.errors,
                ara_diag::Severity::Warning => view.warnings,
                _ => view.info,
            })
            .filter(|e| {
                query.is_empty()
                    || format!("{} {}", e.diagnostic.code, e.diagnostic.message)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect();
        // Collapse keeps the newest row of each code and sums its occurrences.
        let mut totals: BTreeMap<String, u64> = BTreeMap::new();
        if view.collapse {
            for e in &rows {
                *totals.entry(e.diagnostic.code.to_string()).or_default() += e.count;
            }
            let mut seen = std::collections::BTreeSet::new();
            rows.reverse();
            rows.retain(|e| seen.insert(e.diagnostic.code.to_string()));
            rows.reverse();
        }
        if rows.is_empty() {
            self.empty(
                out,
                "console-empty",
                "terminal",
                if all.is_empty() {
                    "Console is empty"
                } else {
                    "No matching messages"
                },
                "Engine, import, Play, script and agent messages appear here as they happen.",
                &[],
            );
            return;
        }
        out.push_str("<scroll id='console-scroll' class='body log-list'>");
        for e in rows.iter().rev().take(300).rev() {
            let id = format!("log-{}", e.seq);
            let (icon, tint) = severity_icon(e.diagnostic.severity);
            let open = self.console_open == Some(e.seq);
            let select = format!("{id}-row");
            self.bind_command(&select, "console.select", Target::Text(e.seq.to_string()));
            let count = if view.collapse {
                totals
                    .get(&e.diagnostic.code.to_string())
                    .copied()
                    .unwrap_or(e.count)
            } else {
                e.count
            };
            out.push_str(&format!("<panel id='{id}' class='log-row{}'><button id='{select}' class='log-main' label='{}' description='Show details' action='Dispatch'>{}<text id='{id}-text' class='log-text' text='{}' />{}</button><panel id='{id}-meta' class='log-meta'><text id='{id}-time' class='log-time' text='{}' />",
                if open { " log-open" } else { "" },
                escape(&e.diagnostic.message),
                icons::icon_in(&id, icon, tint),
                escape(&e.diagnostic.message),
                if count > 1 { format!("<text id='{id}-count' class='log-count' text='×{count}' />") } else { String::new() },
                clock(e.at),
            ));
            // The origin when it is not the engine itself: an agent's name, or the
            // process (import, play) that reported it.
            if e.origin.starts_with("agent") {
                out.push_str(&format!(
                    "<text id='{id}-origin' class='chip-accent' text='{}' />",
                    escape(&crate::activity::short_author(&e.origin))
                ));
            } else if !matches!(e.origin.as_str(), "engine" | "editor" | "") {
                out.push_str(&format!(
                    "<text id='{id}-origin' class='caption' text='{}' />",
                    escape(&e.origin)
                ));
            }
            if let Some((label, binding)) = source(editor, &e.diagnostic) {
                let link = format!("{id}-source");
                self.bindings
                    .insert(ara_ui::source::stable_id(&link), binding);
                out.push_str(&format!("<button id='{link}' class='log-source' label='{}' description='Go to {}' action='Dispatch'><text id='{link}-label' text='{}' /></button>",escape(&label),escape(&label),escape(&label)));
            }
            out.push_str("</panel></panel>");
            if open {
                self.log_details(out, &id, &e.diagnostic);
            }
        }
        out.push_str("</scroll>");
    }

    /// The expanded part of a Console row or a Problems "Why": code, full message,
    /// subjects, the catalog's explanation and the fixes as buttons.
    fn log_details(&mut self, out: &mut String, id: &str, d: &ara_diag::Diagnostic) {
        out.push_str(&format!("<panel id='{id}-details' class='log-details'><text id='{id}-code' class='mono caption' text='{}' />",escape(&d.code.to_string())));
        if let Some(doc) = d.code.doc() {
            out.push_str(&format!("<text id='{id}-title' class='fg' text='{}' /><text id='{id}-explain' class='dialog-text' text='{}' />",escape(doc.title),escape(doc.explanation)));
            if !doc.causes.is_empty() {
                out.push_str(&format!(
                    "<text id='{id}-causes' class='caption' text='Common causes' />"
                ));
                for (n, cause) in doc.causes.iter().enumerate().take(4) {
                    out.push_str(&format!(
                        "<text id='{id}-cause-{n}' class='dialog-text' text='• {}' />",
                        escape(cause)
                    ));
                }
            }
        }
        if !d.subjects.is_empty() {
            let subjects = d
                .subjects
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("  ");
            out.push_str(&format!(
                "<text id='{id}-subjects' class='mono caption' text='{}' />",
                escape(&subjects)
            ));
        }
        if !d.fixes.is_empty() {
            out.push_str(&format!(
                "<panel id='{id}-fixes' class='row' style='gap:6px'>"
            ));
            for (index, fix) in d.fixes.iter().enumerate() {
                out.push_str(&self.bind_button(
                    &format!("{id}-fix-{index}"),
                    &fix.title,
                    Binding::Fix(d.clone(), index),
                    "primary",
                ));
            }
            out.push_str("</panel>");
        }
        out.push_str("</panel>");
    }

    pub(super) fn problems_panel(&mut self, editor: &mut Editor, out: &mut String) {
        let view = self.log_view;
        let all = entries(editor, 0, "warning");
        let errors = all
            .iter()
            .filter(|e| e.diagnostic.severity == ara_diag::Severity::Error)
            .count();
        let warnings = all.len() - errors;
        out.push_str("<panel id='problems-toolbar' class='toolbar'>");
        for (id, command, severity, on, n) in [
            (
                "problems-errors",
                "problems.errors",
                ara_diag::Severity::Error,
                view.problem_errors,
                errors,
            ),
            (
                "problems-warnings",
                "problems.warnings",
                ara_diag::Severity::Warning,
                view.problem_warnings,
                warnings,
            ),
        ] {
            let (icon, tint) = severity_icon(severity);
            self.bind_command(id, command, Target::None);
            out.push_str(&format!("<button id='{id}' class='{} count-toggle' label='{}' description='Show or hide them' action='Dispatch'>{}<text id='{id}-n' text='{n}' /></button>",if on{"segment-on"}else{"ghost"},if severity==ara_diag::Severity::Error{"Errors"}else{"Warnings"},icons::icon_in(id,icon,if on{tint}else{"muted"})));
        }
        self.search(out, "problems", editor);
        out.push_str("</panel>");
        let query = editor
            .state
            .searches
            .get("problems")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        // Group by file or document; problems with neither go under "Project".
        let mut groups: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
        for e in all.iter().rev() {
            let shown = match e.diagnostic.severity {
                ara_diag::Severity::Error => view.problem_errors,
                _ => view.problem_warnings,
            };
            if !shown
                || !(query.is_empty()
                    || format!("{} {}", e.diagnostic.code, e.diagnostic.message)
                        .to_lowercase()
                        .contains(&query))
            {
                continue;
            }
            let group = e
                .diagnostic
                .location
                .file
                .as_deref()
                .map(|file| project_file(editor, file))
                .or_else(|| {
                    e.diagnostic.location.doc.as_deref().and_then(|doc| {
                        editor
                            .host
                            .store()
                            .workspace()
                            .path_of(doc)
                            .map(str::to_owned)
                    })
                })
                .unwrap_or_else(|| "Project".into());
            groups.entry(group).or_default().push(e);
        }
        if groups.is_empty() {
            self.empty(out, "problems-empty", "circle-check", if all.is_empty() { "No problems" } else { "No matching problems" }, "Errors and warnings from documents, imports, scripts and Play are listed here by file, with fixes.", &[]);
            return;
        }
        out.push_str("<scroll id='problems-scroll' class='body log-list'>");
        for (n, (group, rows)) in groups.iter().enumerate() {
            let gid = format!("problem-group-{n}");
            out.push_str(&format!("<panel id='{gid}' class='problem-group'>{}<text id='{gid}-path' class='fg' text='{}' /><text id='{gid}-count' class='caption' text='{}' /></panel>",icons::icon(&gid,if group=="Project"{"folder"}else{"file"}),escape(group),rows.len()));
            for e in rows.iter().take(200) {
                let id = format!("problem-{}", e.seq);
                let (icon, tint) = severity_icon(e.diagnostic.severity);
                let why = format!("{id}-why");
                let explained = self.explained_problems.contains(&e.seq);
                self.bind_command(&why, "problems.explain", Target::Text(e.seq.to_string()));
                // Line 1: the message at the full width. Line 2: code · source, with
                // Go and Why at its right end.
                out.push_str(&format!("<panel id='{id}' class='problem-row'><panel id='{id}-line' class='problem-line'>{}<text id='{id}-text' class='problem-text' text='{}' /></panel><panel id='{id}-meta' class='problem-meta'><text id='{id}-diag' class='log-time' text='{}' />",
                    icons::icon_in(&id, icon, tint),
                    escape(&e.diagnostic.message),
                    escape(&e.diagnostic.code.to_string()),
                ));
                let target = source(editor, &e.diagnostic);
                if let Some((label, _)) = &target {
                    out.push_str(&format!(
                        "<text id='{id}-source' class='problem-source' text='{}' />",
                        escape(label)
                    ));
                }
                out.push_str(&format!("<panel id='{id}-gap' class='fill' />"));
                if let Some((label, binding)) = target {
                    let go = format!("{id}-go");
                    self.bindings
                        .insert(ara_ui::source::stable_id(&go), binding);
                    out.push_str(&format!("<button id='{go}' class='ghost icon-button problem-action' label='Go to {}' description='Go to {}' action='Dispatch'>{}</button>",escape(&label),escape(&label),icons::icon(&go,"crosshair")));
                }
                out.push_str(&format!("<button id='{why}' class='{} icon-button problem-action' label='Why' description='Why: explain this problem' action='Dispatch'>{}</button></panel></panel>",if explained{"selected"}else{"ghost"},if explained{icons::icon_in(&why,"lightbulb","on-selection")}else{icons::icon(&why,"lightbulb")}));
                if explained || !e.diagnostic.fixes.is_empty() {
                    if explained {
                        self.log_details(out, &id, &e.diagnostic);
                    } else {
                        out.push_str(&format!("<panel id='{id}-fixes' class='problem-fixes'>"));
                        for (index, fix) in e.diagnostic.fixes.iter().enumerate().take(3) {
                            out.push_str(&self.bind_button(
                                &format!("{id}-fix-{index}"),
                                &fix.title,
                                Binding::Fix(e.diagnostic.clone(), index),
                                "small",
                            ));
                        }
                        out.push_str("</panel>");
                    }
                }
            }
        }
        out.push_str("</scroll>");
    }
}
