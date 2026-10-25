//! The Profiler page (Unreal Insights' Timing view, Unity's Profiler): a frame strip
//! against the frame budget, the selected frame as thread lanes plus GPU passes, its
//! scopes as a table beside the frame's counters, and two-capture comparisons. The
//! data is `ara-profile`'s, the same captures `ara profile` and agents read.
//!
//! Timeline geometry is in percentages of the frame (spacers and spans side by side),
//! so it never waits for a measured width; only label fitting reads the last layout.
use super::*;
use ara_profile::{CpuSpan, Frame, Profile};

/// Frames the strip shows (the newest).
const STRIP: usize = 120;
/// Height of the frame strip, in logical pixels.
const STRIP_HEIGHT: f32 = 56.;
/// Spans shorter than this share of the frame are left out of the timeline.
const MIN_SHARE: f64 = 0.0015;
/// Most spans drawn per frame; the rest still count in the Scopes table.
const MAX_SPANS: usize = 480;

impl Shell {
    pub(super) fn profiler_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        // Read the editor state first: the live profile borrows the host mutably.
        let recording = editor.host.profiler().is_recording();
        let frozen = editor.profile_current.is_some();
        let baseline = editor.profile_baseline.clone();
        let compare = editor.state.profiler_compare;
        let picked = editor.state.profiler_frame;
        let current = match &editor.profile_current {
            Some(profile) => profile.as_ref(),
            None => editor.host.profiler().profile(),
        };
        let selected = picked
            .and_then(|id| current.frames.iter().find(|f| f.id == id))
            .or_else(|| current.frames.iter().rev().find(|f| f.duration_ns > 0));
        out.push_str("<panel id='profiler-toolbar' class='toolbar'>");
        self.bind_command("profile-record", "profiler.record", commands::Target::None);
        out.push_str(&format!("<button id='profile-record' class='{}' label='{}' description='{}' action='Dispatch'>{}<text id='profile-record-label' text='{}' /></button>",if recording{"selected"}else{"ghost"},if recording{"Stop"}else{"Record"},if recording{"Stop recording editor frames"}else{"Record editor frames"},if recording{icons::icon_in("profile-record","square","on-selection")}else{icons::icon_in("profile-record","circle-dot","destructive")},if recording{"Stop"}else{"Record"}));
        self.tool_text(
            out,
            "profile-freeze",
            "profiler.freeze",
            "pause",
            "Freeze",
            "ghost",
        );
        self.tool_text(
            out,
            "profile-baseline",
            "profiler.baseline",
            "flag",
            "Baseline",
            "ghost",
        );
        self.bind_command(
            "profile-compare",
            "profiler.compare",
            commands::Target::None,
        );
        out.push_str(&format!("<button id='profile-compare' class='{}' label='Compare' description='Compare the frozen capture with the baseline' action='Dispatch'>{}<text id='profile-compare-label' text='Compare' /></button>",if compare{"selected"}else{"ghost"},if compare{icons::icon_in("profile-compare","git-compare","on-selection")}else{icons::icon("profile-compare","git-compare")}));
        if let Some(frame) = selected.filter(|_| !compare) {
            // ‹ Frame 239 › steps through the capture.
            let at = current
                .frames
                .iter()
                .position(|f| f.id == frame.id)
                .unwrap_or(0);
            out.push_str("<panel id='profile-step-sep' class='toolbar-sep' />");
            for (id, icon, label, step) in [
                (
                    "profile-prev",
                    "chevron-left",
                    "Previous frame",
                    at.checked_sub(1),
                ),
                ("profile-next", "chevron-right", "Next frame", Some(at + 1)),
            ] {
                let target = step
                    .and_then(|i| current.frames.get(i))
                    .map_or(frame.id, |f| f.id);
                self.bindings.insert(
                    ara_ui::source::stable_id(id),
                    Binding::Protocol("editor.workspace", json!({ "frame": target })),
                );
                out.push_str(&format!("<button id='{id}' class='ghost icon-button' label='{label}' description='{label}' action='Dispatch'>{}</button>",icons::icon(id,icon)));
                if id == "profile-prev" {
                    out.push_str(&format!(
                        "<text id='profile-frame-title' class='fg' text='Frame {}' />",
                        frame.id
                    ));
                }
            }
            let gpu: u64 = frame.gpu.iter().map(|p| p.duration_ns).sum();
            let over = frame.duration_ns > current.budget_ns;
            out.push_str(&format!("<panel id='profile-toolbar-fill' class='fill' /><text id='profile-cpu' class='{}' text='CPU {}' description='Wall time of the frame on the main thread' /><text id='profile-gpu-total' class='fg' text='GPU {}' description='Sum of the GPU pass durations in this frame' /><text id='profile-budget-text' class='caption' text='of {}' description='Frame budget' /></panel>",if over{"warning"}else{"success"},ms(frame.duration_ns),if frame.gpu.is_empty(){"—".into()}else{ms(gpu)},ms(current.budget_ns)));
        } else {
            out.push_str("<panel id='profile-toolbar-fill' class='fill' /></panel>");
        }
        if compare {
            self.profile_compare(baseline.as_deref(), current, out)?;
            profile_status(current, frozen, baseline.is_some(), recording, out);
            return Ok(());
        }
        let Some(frame) = selected else {
            self.empty(
                out,
                "profile-empty",
                "gauge",
                "No frames recorded",
                "Record while you work, then Freeze to inspect thread lanes, GPU passes and hitches.",
                &[("profile-empty-record", "Record", "profiler.record")],
            );
            return Ok(());
        };
        out.push_str("<panel id='profile-content' class='profile-content'>");
        self.profile_strip(current, frame, out);
        self.profile_timeline(current, frame, out);
        out.push_str("<panel id='profile-lower-edge' class='profile-edge' /><panel id='profile-lower' class='profile-lower'>");
        self.profile_scopes(current, frame, out);
        out.push_str("<panel id='profile-details-edge' class='tool-edge' />");
        profile_details(frame, out);
        out.push_str("</panel></panel>");
        profile_status(current, frozen, baseline.is_some(), recording, out);
        Ok(())
    }

    /// One bar per frame (height ∝ duration), the budget as a labelled line; click
    /// selects a frame. Hitches are red, the selection is the accent.
    fn profile_strip(&mut self, current: &Profile, selected: &Frame, out: &mut String) {
        let budget = current.budget_ns.max(1);
        let frames = current
            .frames
            .iter()
            .skip(current.frames.len().saturating_sub(STRIP));
        let top = frames
            .clone()
            .map(|f| f.duration_ns)
            .max()
            .unwrap_or(0)
            .max(budget + budget / 4);
        out.push_str("<panel id='profile-strip' class='profile-strip'>");
        for frame in frames {
            let id = format!("profile-frame-{}", frame.id);
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::Protocol("editor.workspace", json!({"frame":frame.id})),
            );
            let class = if frame.id == selected.id {
                " profile-bar-on"
            } else if frame.duration_ns > budget {
                " profile-bar-hitch"
            } else {
                ""
            };
            out.push_str(&format!("<button id='{id}' class='profile-bar{class}' style='height:{:.1}%' label='Frame {}: {}' description='Frame {}: {}' action='Dispatch' />",(frame.duration_ns as f64/top as f64*100.).max(3.),frame.id,ms(frame.duration_ns),frame.id,ms(frame.duration_ns)));
        }
        let y = STRIP_HEIGHT * (1. - budget as f32 / top as f32);
        out.push_str(&format!("<panel id='profile-budget' class='profile-budget' style='top:{y:.1}px' /><text id='profile-budget-label' class='profile-budget-label' style='top:{:.1}px' text='{} budget' /></panel>",(y-14.).max(0.),ms(budget)));
    }

    /// The selected frame on a time ruler: one lane per CPU thread (spans nested by
    /// depth, coloured by name) and one per GPU queue (pass durations end to end:
    /// GPU timestamps are not aligned to the CPU clock).
    fn profile_timeline(&mut self, current: &Profile, frame: &Frame, out: &mut String) {
        let duration = frame.duration_ns.max(1) as f64;
        let width = self
            .ui
            .find("profile-ruler-track")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(self.ui.size[0] * 0.7);
        // Ruler: the largest step that keeps labels ~90 px apart.
        let step = [
            0.05, 0.1, 0.2, 0.5, 1., 2., 5., 10., 20., 50., 100., 200., 500.,
        ]
        .into_iter()
        .find(|s| s * 1e6 / duration * f64::from(width) >= 90.)
        .unwrap_or(1000.);
        out.push_str("<panel id='profile-ruler' class='profile-ruler'><panel id='profile-ruler-gap' class='profile-lane-name' /><panel id='profile-ruler-track' class='profile-track'>");
        let share = step * 1e6 / duration * 100.;
        let ticks = (100. / share).ceil() as usize;
        for n in 0..ticks.min(64) {
            out.push_str(&format!("<panel id='profile-tick-{n}' class='profile-tick' style='width:{:.3}%'><panel id='profile-tick-{n}-line' class='profile-tick-line' /><text id='profile-tick-{n}-label' class='caption' text='{}' /></panel>",share.min(100.-share*n as f64).max(0.),tick(step*n as f64)));
        }
        out.push_str("</panel></panel><scroll id='profile-lanes' class='profile-lanes'>");
        // Lanes: threads in order of their first scope (the main thread opens the frame).
        let parents: BTreeMap<(u32, u64), u64> = frame
            .spans
            .iter()
            .map(|s| ((s.thread, s.id), s.parent))
            .collect();
        let mut lanes: BTreeMap<u32, Vec<Vec<&CpuSpan>>> = BTreeMap::new();
        let mut first: BTreeMap<u32, u64> = BTreeMap::new();
        for span in &frame.spans {
            let mut depth = 0;
            let mut parent = span.parent;
            while parent != 0 && depth < 12 {
                depth += 1;
                parent = parents.get(&(span.thread, parent)).copied().unwrap_or(0);
            }
            let rows = lanes.entry(span.thread).or_default();
            if rows.len() <= depth {
                rows.resize(depth + 1, Vec::new());
            }
            rows[depth].push(span);
            let at = first.entry(span.thread).or_insert(span.start_ns);
            *at = (*at).min(span.start_ns);
        }
        for rows in lanes.values_mut() {
            for row in rows {
                row.sort_by_key(|s| s.start_ns);
            }
        }
        let mut order: Vec<u32> = lanes.keys().copied().collect();
        order.sort_by_key(|t| (first[t], *t));
        let mut drawn = 0;
        for thread in order {
            let name = current
                .threads
                .iter()
                .find(|t| t.id == thread)
                .map_or_else(|| format!("Thread {thread}"), |t| t.name.clone());
            let lane = format!("profile-lane-{thread}");
            out.push_str(&format!("<panel id='{lane}' class='profile-lane'><panel id='{lane}-name' class='profile-lane-name'><text id='{lane}-label' class='fg' text='{}' /></panel><panel id='{lane}-rows' class='profile-rows'>",escape(&name)));
            for (depth, row) in lanes[&thread].iter().enumerate() {
                out.push_str(&format!(
                    "<panel id='{lane}-row-{depth}' class='profile-row'>"
                ));
                let mut cursor = 0.;
                for span in row.iter() {
                    let share = span.duration_ns as f64 / duration;
                    if share < MIN_SHARE || drawn >= MAX_SPANS {
                        continue;
                    }
                    let start = (span.start_ns.saturating_sub(frame.start_ns) as f64 / duration)
                        .max(cursor);
                    let share = share.min(1. - start);
                    if share <= 0. {
                        continue;
                    }
                    if start > cursor {
                        out.push_str(&format!(
                            "<panel id='pgap-{drawn}' class='profile-gap' style='width:{:.3}%' />",
                            (start - cursor) * 100.
                        ));
                    }
                    let label = current
                        .labels
                        .get(span.label as usize)
                        .map_or("scope", String::as_str);
                    self.profile_span(
                        out,
                        drawn,
                        label,
                        share,
                        width,
                        &format!(
                            "{label} · total {} · self {}",
                            ms(span.duration_ns),
                            ms(span.self_ns)
                        ),
                    );
                    cursor = start + share;
                    drawn += 1;
                }
                out.push_str("</panel>");
            }
            out.push_str(&format!(
                "</panel></panel><panel id='{lane}-edge' class='profile-lane-edge' />"
            ));
        }
        let mut queues: BTreeMap<u32, Vec<&ara_profile::GpuPass>> = BTreeMap::new();
        for pass in &frame.gpu {
            queues.entry(pass.queue).or_default().push(pass);
        }
        for (queue, passes) in queues {
            // A queue whose passes are all too short to draw gets no lane.
            if passes
                .iter()
                .all(|p| (p.duration_ns as f64 / duration) < MIN_SHARE)
            {
                continue;
            }
            let lane = format!("profile-gpu-{queue}");
            let name = current
                .labels
                .get(queue as usize)
                .map_or("GPU", String::as_str);
            out.push_str(&format!("<panel id='{lane}' class='profile-lane'><panel id='{lane}-name' class='profile-lane-name' description='GPU pass durations laid end to end; device timestamps are not aligned to the CPU clock'><text id='{lane}-label' class='fg' text='GPU' /><text id='{lane}-queue' class='caption' text='{}' /></panel><panel id='{lane}-rows' class='profile-rows'><panel id='{lane}-row' class='profile-row'>",escape(name)));
            let mut cursor = 0.;
            for pass in passes {
                let share = (pass.duration_ns as f64 / duration).min(1. - cursor);
                if share < MIN_SHARE || drawn >= MAX_SPANS {
                    continue;
                }
                let label = current
                    .labels
                    .get(pass.label as usize)
                    .map_or("pass", String::as_str);
                self.profile_span(
                    out,
                    drawn,
                    label,
                    share,
                    width,
                    &format!("{label} · {}", ms(pass.duration_ns)),
                );
                cursor += share;
                drawn += 1;
            }
            out.push_str(&format!(
                "</panel></panel></panel><panel id='{lane}-edge' class='profile-lane-edge' />"
            ));
        }
        out.push_str("</scroll>");
    }

    /// One timeline block, coloured by a stable hash of its name, labelled when the
    /// name fits; spans of the scope picked in the Scopes table are outlined.
    fn profile_span(
        &self,
        out: &mut String,
        n: usize,
        label: &str,
        share: f64,
        width: f32,
        description: &str,
    ) {
        let colour = label.bytes().fold(2166136261u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(16777619)
        }) % 6
            + 1;
        let picked = if self.profile_scope.as_deref() == Some(label) {
            " pspan-on"
        } else {
            ""
        };
        // ~6.5 px per character at 11 px, plus the block's padding.
        let fits = share * f64::from(width) >= label.chars().count() as f64 * 6.5 + 10.;
        out.push_str(&format!("<panel id='pspan-{n}' class='pspan pc{colour}{picked}' style='width:{:.3}%' description='{}'>",share*100.,escape(description)));
        if fits {
            out.push_str(&format!(
                "<text id='pspan-{n}-label' class='pspan-label' text='{}' />",
                escape(label)
            ));
        }
        out.push_str("</panel>");
    }

    /// The frame's scopes by name: calls, total (inclusive), self (exclusive) and
    /// their share of the frame, most self time first. A row outlines its spans.
    fn profile_scopes(&mut self, current: &Profile, frame: &Frame, out: &mut String) {
        let mut scopes: BTreeMap<&str, (u64, u64, u64)> = BTreeMap::new();
        for span in &frame.spans {
            let label = current
                .labels
                .get(span.label as usize)
                .map_or("scope", String::as_str);
            let entry = scopes.entry(label).or_default();
            entry.0 += 1;
            entry.1 += span.duration_ns;
            entry.2 += span.self_ns;
        }
        let mut rows: Vec<_> = scopes.into_iter().collect();
        rows.sort_by(|a, b| b.1.2.cmp(&a.1.2).then(a.0.cmp(b.0)));
        out.push_str("<panel id='profile-scopes' class='profile-scopes'><panel id='profile-scopes-head' class='profile-table-head'><text id='profile-scopes-title' class='heading fill' text='Scopes' /><text id='profile-head-calls' class='caption profile-col-calls' text='Calls' /><text id='profile-head-total' class='caption profile-col-time' text='Total' /><text id='profile-head-self' class='caption profile-col-time' text='Self' /><text id='profile-head-share' class='caption profile-col-share' text='Share' /></panel><scroll id='profile-scope-list' class='profile-table'>");
        let duration = frame.duration_ns.max(1) as f64;
        for (n, (label, (calls, total, own))) in rows.iter().enumerate().take(200) {
            let id = format!("pscope-{n}");
            self.bind_command(
                &id,
                "profiler.scope",
                commands::Target::Text((*label).to_owned()),
            );
            let share = *own as f64 / duration * 100.;
            let on = self.profile_scope.as_deref() == Some(*label);
            out.push_str(&format!("<button id='{id}' class='profile-scope-row{}' label='{}' description='Outline {} in the timeline' action='Dispatch'><text id='{id}-name' class='fill' style='overflow:hidden' text='{}' /><text id='{id}-calls' class='number profile-col-calls' text='{calls}' /><text id='{id}-total' class='number profile-col-time' text='{}' /><text id='{id}-self' class='number profile-col-time' text='{}' /><panel id='{id}-share' class='profile-col-share profile-share'><panel id='{id}-bar' class='profile-share-track'><panel id='{id}-fill' class='profile-share-fill' style='width:{:.1}%' /></panel><text id='{id}-pct' class='number caption' text='{:.0}%' /></panel></button>",if on{" profile-scope-on"}else{""},escape(label),escape(label),escape(label),ms(*total),ms(*own),share.min(100.),share));
        }
        out.push_str("</scroll></panel>");
    }

    /// Baseline against the frozen capture: a verdict, then every metric's p95 with its
    /// change as a diverging bar, regressions first.
    fn profile_compare(
        &mut self,
        baseline: Option<&Profile>,
        current: &Profile,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let Some(baseline) = baseline else {
            self.empty(
                out,
                "profile-baseline-empty",
                "git-compare",
                "No baseline yet",
                "Set a baseline, record another capture, then freeze it to compare the two.",
                &[("profile-baseline-set", "Set baseline", "profiler.baseline")],
            );
            return Ok(());
        };
        let budget = ara_profile::SummaryBudget {
            top_n: 1024,
            hitches: 64,
            max_bytes: 1_048_576,
        };
        let before = baseline.summary(budget).map_err(String::from)?;
        let after = current.summary(budget).map_err(String::from)?;
        let comparison = ara_profile::compare(&before, &after, 0.05, 1000).map_err(String::from)?;
        let regressions = comparison.deltas.iter().filter(|d| d.regression).count();
        let mut deltas: Vec<_> = comparison.deltas.iter().collect();
        deltas.sort_by(|a, b| {
            b.regression.cmp(&a.regression).then(
                b.change
                    .unwrap_or(f64::INFINITY)
                    .abs()
                    .total_cmp(&a.change.unwrap_or(f64::INFINITY).abs()),
            )
        });
        let (icon, tint, verdict) = if comparison.passed {
            (
                "circle-check",
                "success",
                "Within the regression budget".to_owned(),
            )
        } else if regressions > 0 {
            (
                "triangle-alert",
                "warning",
                format!(
                    "{regressions} regression{}",
                    if regressions == 1 { "" } else { "s" }
                ),
            )
        } else {
            (
                "triangle-alert",
                "warning",
                comparison
                    .issues
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Review the changed metrics".into()),
            )
        };
        out.push_str(&format!("<panel id='profile-compare-head' class='profile-compare-head'>{}<text id='profile-compare-verdict' class='{tint}' text='{}' /><panel id='profile-compare-fill' class='fill' /><text id='profile-compare-meta' class='caption' text='p95 frame {} → {} · {} → {} frames · regression past 5% and 1 µs' /></panel>",icons::icon_in("profile-compare-icon",icon,tint),escape(&verdict),ms(before.cpu_frame.p95),ms(after.cpu_frame.p95),before.frames,after.frames));
        out.push_str("<panel id='profile-compare-columns' class='profile-table-head'><text id='profile-metric-heading' class='caption fill' text='Metric (p95)' /><text id='profile-baseline-heading' class='caption profile-col-time' text='Baseline' /><text id='profile-current-heading' class='caption profile-col-time' text='Current' /><text id='profile-delta-heading' class='caption profile-col-time' text='Change' /><panel id='profile-delta-bar-heading' class='profile-col-delta' /></panel><scroll id='profile-compare-list' class='profile-table'>");
        for (n, delta) in deltas.iter().take(200).enumerate() {
            let id = format!("profile-delta-{n}");
            let change = delta.change.unwrap_or(0.);
            let (class, text) = match delta.change {
                None => ("warning", "New".to_owned()),
                Some(_) if delta.regression => ("warning", format!("{:+.1}%", change * 100.)),
                Some(c) if c < 0. => ("success", format!("{:+.1}%", c * 100.)),
                Some(c) => ("caption", format!("{:+.1}%", c * 100.)),
            };
            // Diverging bar: faster grows left (success), slower right; ±50 % fills it.
            let reach = (change.abs() * 200.).min(100.);
            let (left, right) = if change < 0. {
                (reach, 0.)
            } else {
                (0., reach)
            };
            out.push_str(&format!("<panel id='{id}' class='profile-delta-row{}'><text id='{id}-key' class='fill' style='overflow:hidden' text='{}' /><text id='{id}-before' class='number profile-col-time' text='{}' /><text id='{id}-after' class='number profile-col-time' text='{}' /><text id='{id}-change' class='number profile-col-time {class}' text='{text}' /><panel id='{id}-bar' class='profile-col-delta profile-delta-bar'><panel id='{id}-left' class='profile-delta-half profile-delta-left'><panel id='{id}-better' class='profile-delta-better' style='width:{left:.1}%' /></panel><panel id='{id}-axis' class='profile-delta-axis' /><panel id='{id}-right' class='profile-delta-half'><panel id='{id}-worse' class='{}' style='width:{right:.1}%' /></panel></panel></panel>",if delta.regression{" profile-delta-regression"}else{""},escape(&delta.key),opt_ms(delta.baseline_ns),opt_ms(delta.current_ns),if delta.regression{"profile-delta-worse"}else{"profile-delta-slower"}));
        }
        out.push_str("</scroll>");
        Ok(())
    }
}

/// The selected frame's counters and memory, as label/value rows.
fn profile_details(frame: &Frame, out: &mut String) {
    let gpu: u64 = frame.gpu.iter().map(|p| p.duration_ns).sum();
    let count = |n: u64| group_digits(n);
    let groups: [(&str, Vec<(&str, String)>); 3] = [
        (
            "Frame",
            vec![
                ("Wall time", ms(frame.duration_ns)),
                ("GPU passes", format!("{} · {}", frame.gpu.len(), ms(gpu))),
                ("Scopes", count(frame.spans.len() as u64)),
                ("Jobs", count(frame.jobs.len() as u64)),
            ],
        ),
        (
            "World",
            vec![
                ("Entities", count(frame.counters.entities)),
                ("Draws", count(frame.counters.draws)),
                ("Triangles", count(frame.counters.triangles)),
                ("Particles", count(frame.counters.particles)),
                ("Physics bodies", count(frame.counters.physics_bodies)),
                ("Audio voices", count(frame.counters.voices)),
            ],
        ),
        (
            "Memory",
            vec![
                ("CPU heap", bytes(frame.memory.cpu_heap)),
                ("Allocated", bytes(frame.memory.allocated_bytes)),
                ("Allocations", count(frame.memory.allocations)),
                ("GPU textures", bytes(frame.memory.gpu_textures)),
                ("GPU buffers", bytes(frame.memory.gpu_buffers)),
            ],
        ),
    ];
    out.push_str(&format!("<scroll id='profile-details' class='tool-side profile-details'><panel id='profile-details-head' class='side-header'><text id='profile-details-title' class='heading' text='Frame {}' /><panel id='profile-details-fill' class='fill' />{}</panel>",frame.id,if frame.dropped>0{format!("<text id='profile-dropped' class='warning' text='{} lost' description='Ring, nesting or capacity losses: this frame is incomplete' />",frame.dropped)}else{String::new()}));
    for (group, rows) in groups {
        let gid = format!("profile-details-{}", group.to_lowercase());
        out.push_str(&format!(
            "<text id='{gid}' class='profile-group caption' text='{}' />",
            group.to_uppercase()
        ));
        for (n, (label, value)) in rows.iter().enumerate() {
            out.push_str(&format!("<panel id='{gid}-{n}' class='profile-stat'><text id='{gid}-{n}-label' class='caption' text='{label}' /><panel id='{gid}-{n}-fill' class='fill' /><text id='{gid}-{n}-value' class='number fg' text='{}' /></panel>",escape(value)));
        }
    }
    out.push_str("</scroll>");
}

/// What the page shows and the capture's shape: frames, hitches, percentiles.
fn profile_status(
    current: &Profile,
    frozen: bool,
    baseline: bool,
    recording: bool,
    out: &mut String,
) {
    let (icon, tint, state) = if recording {
        ("circle-dot", "destructive", "Recording")
    } else if frozen {
        ("pause", "fg", "Frozen capture")
    } else {
        ("gauge", "fg", "Live")
    };
    let mut times: Vec<u64> = current
        .frames
        .iter()
        .map(|f| f.duration_ns)
        .filter(|d| *d > 0)
        .collect();
    times.sort_unstable();
    let pick = |q: f64| {
        times
            .get(((times.len().max(1) - 1) as f64 * q).round() as usize)
            .copied()
            .unwrap_or(0)
    };
    let hitches = times.iter().filter(|d| **d > current.budget_ns).count();
    out.push_str(&format!("<panel id='profiler-status' class='tool-status'>{}<text id='profiler-status-state' class='{tint}' text='{state}' /><panel id='profiler-status-fill' class='fill' /><text id='profiler-status-stats' class='caption' text='{} frames · {hitches} hitch{} · p50 {} · p95 {}{}' /></panel>",icons::icon_in("profiler-status-icon",icon,tint),times.len(),if hitches==1{""}else{"es"},ms(pick(0.5)),ms(pick(0.95)),if baseline{" · baseline set"}else{""}));
}

/// Nanoseconds as people read frame times: "4.90 ms", "312 µs".
fn ms(ns: u64) -> String {
    if ns >= 1_000_000 {
        format!("{:.2} ms", ns as f64 / 1e6)
    } else {
        format!("{} µs", (ns as f64 / 1e3).round())
    }
}

fn opt_ms(ns: Option<u64>) -> String {
    ns.map_or_else(|| "—".into(), ms)
}

/// A ruler label: "0", "0.5 ms", "2 ms".
fn tick(value_ms: f64) -> String {
    if value_ms == 0. {
        "0".into()
    } else {
        format!("{} ms", (value_ms * 100.).round() / 100.)
    }
}

/// 1,204,512: comma digit groups.
fn group_digits(n: u64) -> String {
    let text = n.to_string();
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i > 0 && (text.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Bytes in binary units: "512 B", "3.2 MB".
fn bytes(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.),
        1_048_576..1_073_741_824 => format!("{:.1} MB", n as f64 / 1_048_576.),
        _ => format!("{:.2} GB", n as f64 / 1_073_741_824.),
    }
}
