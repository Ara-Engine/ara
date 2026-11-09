//! The Profiler (Unity's Profiler, Unreal Insights' timing view): Record starts the same
//! bounded capture agents read (`profile.start`); a strip of the newest frames against the
//! frame budget, click one to read it; that frame's threads as lanes of nested scopes; its
//! scopes ranked by self time; and its GPU passes. The data is `ara-profile`'s, copied into
//! `Studio::profile` a few times a second while recording.
use std::collections::BTreeMap;

use ara_kit::icons;
use ara_kit::ui::*;
use ara_profile::{Frame, Profile};
use serde_json::json;
use stillkit::Rgba;
use stillkit::el::{Align, Axis, El, Justify, col, el, row, text};

use super::Studio;

/// Frames the strip shows (the newest).
const STRIP: usize = 120;
/// One nesting level's height in a lane.
const DEPTH: f32 = 15.0;
/// Scopes shorter than this share of the frame are left out of the lanes.
const MIN_SHARE: f64 = 0.002;

/// The panel.
pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let profile = &*s.profile;
    let recording = s.local.profiling;
    let frame = s
        .local
        .profile_frame
        .and_then(|id| profile.frames.iter().find(|f| f.id == id))
        .or(profile.frames.back());
    let toolbar = row()
        .h(super::style::TOOLBAR + 6.0)
        .pad_x(6.0)
        .gap(8.0)
        .align(Align::Center)
        .child(if recording {
            button(l, Tone::Danger, Size::Sm, Some(icons::CIRCLE_STOP), "Stop")
                .key("profiler/stop")
                .on_press(|s: &mut Studio| s.act("profile.stop", json!({})))
        } else {
            button(
                l,
                Tone::Secondary,
                Size::Sm,
                Some(icons::CIRCLE_DOT),
                "Record",
            )
            .key("profiler/record")
            .on_press(|s: &mut Studio| {
                s.local.profile_frame = None;
                s.act("profile.start", json!({}))
            })
        })
        .child(
            text(format!(
                "{} · {} frames · budget {}",
                if recording { "Recording" } else { "Stopped" },
                profile.frames.len(),
                ms(profile.budget_ns)
            ))
            .text_size(11.5)
            .color(c.muted_foreground.0),
        )
        .child(el().grow(1.0))
        .children(frame.map(|f| {
            text(format!("Frame {} · {}", f.id, ms(f.duration_ns)))
                .text_size(11.5)
                .family(mono())
                .color(c.foreground.0)
        }));
    let Some(frame) = frame else {
        return col().grow(1.0).child(toolbar).child(
            col().grow(1.0).justify(Justify::Center).child(empty(
                l,
                icons::ACTIVITY,
                "No frames recorded",
                "Record, use the editor or play, then pick a frame to see where its time went.",
                None,
            )),
        );
    };
    let width = s
        .local
        .rects
        .get("profiler/track")
        .map_or(640.0, |r| r.width());
    col()
        .grow(1.0)
        .child(toolbar)
        .child(strip(s, profile, frame.id))
        .child(
            col()
                .h(0.0)
                .grow(1.0)
                .scroll(Axis::Vertical)
                .child(lanes(s, profile, frame, width))
                .child(
                    row()
                        .gap(12.0)
                        .pad(8.0)
                        .align(Align::Start)
                        .child(scopes(s, profile, frame).w(0.0).grow(3.0))
                        .child(gpu(s, profile, frame).w(0.0).grow(2.0)),
                ),
        )
}

/// The newest frames as bars against the budget; a click reads that frame.
fn strip(s: &Studio, profile: &Profile, picked: u64) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let budget = profile.budget_ns.max(1) as f32;
    let start = profile.frames.len().saturating_sub(STRIP);
    let tallest = profile
        .frames
        .iter()
        .skip(start)
        .map(|f| f.duration_ns as f32)
        .fold(budget * 1.5, f32::max);
    let mut bars = row().h(56.0).gap(1.0).align(Align::End).pad_x(8.0);
    for f in profile.frames.iter().skip(start) {
        let over = f.duration_ns as f32 > budget;
        let on = f.id == picked;
        let h = (f.duration_ns as f32 / tallest * 52.0).max(2.0);
        let id = f.id;
        bars = bars.child(
            col()
                .key(format!("profiler/frame/{id}"))
                .w(0.0)
                .grow(1.0)
                .h_full()
                .justify(Justify::End)
                .label(format!("Frame {id}, {}", ms(f.duration_ns)))
                .on_press(move |s: &mut Studio| s.local.profile_frame = Some(id))
                .child(el().h(h).radius(1.0).fill(match (on, over) {
                    (true, _) => c.foreground.0,
                    (false, true) => c.destructive.0.with_alpha(0.8),
                    (false, false) => c.info.0.with_alpha(0.55),
                })),
        );
    }
    // The budget line across the strip.
    let line = (52.0 - budget / tallest * 52.0).max(0.0);
    col()
        .h(60.0)
        .child(
            el().absolute(8.0, line + 4.0)
                .w_full()
                .h(1.0)
                .fill(c.muted_foreground.0.with_alpha(0.35)),
        )
        .child(bars)
}

/// The frame's threads as lanes; each scope sits where it ran, nested scopes below their
/// parents. `width` is the track's laid-out width.
fn lanes(s: &Studio, profile: &Profile, frame: &Frame, width: f32) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let span_ns = frame.duration_ns.max(1) as f64;
    let mut by_thread: BTreeMap<u32, Vec<&ara_profile::CpuSpan>> = BTreeMap::new();
    for span in &frame.spans {
        by_thread.entry(span.thread).or_default().push(span);
    }
    let mut out = col().pad_x(8.0).gap(4.0);
    for (lane, (thread, spans)) in by_thread.into_iter().enumerate() {
        let name = profile
            .threads
            .iter()
            .find(|t| t.id == thread)
            .map_or_else(|| format!("Thread {thread}"), |t| t.name.clone());
        // Depth by parent chain within this thread.
        let mut depth: BTreeMap<u64, usize> = BTreeMap::new();
        let mut placed = Vec::new();
        for span in spans {
            let d = if span.parent == 0 {
                0
            } else {
                depth.get(&span.parent).map_or(0, |p| p + 1)
            };
            depth.insert(span.id, d);
            let share = span.duration_ns as f64 / span_ns;
            if share >= MIN_SHARE {
                placed.push((span, d));
            }
        }
        let rows = placed.iter().map(|(_, d)| *d).max().map_or(1, |d| d + 1);
        // The first lane's width is what the host reports back for the next layout.
        let mut track = el()
            .key(if lane == 0 {
                "profiler/track".to_owned()
            } else {
                format!("profiler/lane/{thread}")
            })
            .w(0.0)
            .grow(1.0)
            .h(rows as f32 * DEPTH);
        for (span, d) in placed {
            let x = (span.start_ns.saturating_sub(frame.start_ns) as f64 / span_ns) as f32 * width;
            let w = ((span.duration_ns as f64 / span_ns) as f32 * width).max(1.0);
            let label = profile
                .labels
                .get(span.label as usize)
                .cloned()
                .unwrap_or_default();
            let tint = hue(l, &label);
            track = track.child(
                el().absolute(x, d as f32 * DEPTH)
                    .w(w)
                    .h(DEPTH - 1.0)
                    .radius(2.0)
                    .fill(tint)
                    .tooltip(format!(
                        "{label} · {} (self {})",
                        ms(span.duration_ns),
                        ms(span.self_ns)
                    ))
                    .children((w > 48.0).then(|| {
                        text(label.clone())
                            .text_size(10.0)
                            .color(Rgba::new(0.0, 0.0, 0.0, 0.85))
                            .pad_x(4.0)
                            .wrap(1)
                    })),
            );
        }
        out = out.child(
            row()
                .gap(8.0)
                .align(Align::Start)
                .child(
                    text(name)
                        .text_size(11.0)
                        .color(c.muted_foreground.0)
                        .w(96.0),
                )
                .child(track),
        );
    }
    out
}

/// The frame's scopes merged by name, ranked by self time.
fn scopes(s: &Studio, profile: &Profile, frame: &Frame) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let mut merged: BTreeMap<u32, (u32, u64, u64)> = BTreeMap::new();
    for span in &frame.spans {
        let e = merged.entry(span.label).or_default();
        e.0 += 1;
        e.1 += span.duration_ns;
        e.2 += span.self_ns;
    }
    let mut ranked: Vec<_> = merged.into_iter().collect();
    ranked.sort_by_key(|(_, (_, _, own))| std::cmp::Reverse(*own));
    let mut table = col()
        .gap(1.0)
        .child(header(l, ["Scope", "Calls", "Total", "Self"]));
    for (label, (calls, total, own)) in ranked.into_iter().take(24) {
        let name = profile
            .labels
            .get(label as usize)
            .cloned()
            .unwrap_or_default();
        table = table.child(
            row()
                .h(22.0)
                .gap(8.0)
                .pad_x(6.0)
                .align(Align::Center)
                .child(el().size(8.0).radius(2.0).fill(hue(l, &name)))
                .child(
                    text(name)
                        .text_size(12.0)
                        .color(c.foreground.0)
                        .w(0.0)
                        .grow(1.0)
                        .wrap(1),
                )
                .child(num(l, calls.to_string()))
                .child(num(l, ms(total)))
                .child(num(l, ms(own))),
        );
    }
    table
}

/// The frame's GPU passes, longest first, with bars against the longest.
fn gpu(s: &Studio, profile: &Profile, frame: &Frame) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let mut passes: Vec<_> = frame.gpu.iter().collect();
    passes.sort_by_key(|p| std::cmp::Reverse(p.duration_ns));
    let longest = passes.first().map_or(1, |p| p.duration_ns.max(1)) as f32;
    let mut out = col()
        .gap(1.0)
        .child(header(l, ["GPU pass", "", "", "Time"]));
    if passes.is_empty() {
        out = out.child(
            row().pad(6.0).child(
                text("No GPU timings in this frame")
                    .text_size(11.5)
                    .color(c.muted_foreground.0),
            ),
        );
    }
    for pass in passes.into_iter().take(24) {
        let name = profile
            .labels
            .get(pass.label as usize)
            .cloned()
            .unwrap_or_default();
        out = out.child(
            row()
                .h(22.0)
                .gap(8.0)
                .pad_x(6.0)
                .align(Align::Center)
                .child(
                    text(name)
                        .text_size(12.0)
                        .color(c.foreground.0)
                        .w(0.0)
                        .grow(1.0)
                        .wrap(1),
                )
                .child(
                    el().w(60.0)
                        .h(6.0)
                        .radius(3.0)
                        .fill(super::style::tones(l).well)
                        .child(
                            el().w(pass.duration_ns as f32 / longest * 60.0)
                                .h(6.0)
                                .radius(3.0)
                                .fill(c.info.0.with_alpha(0.7)),
                        ),
                )
                .child(num(l, ms(pass.duration_ns))),
        );
    }
    out
}

fn header(l: &ara_kit::look::Look, titles: [&str; 4]) -> El<Studio> {
    let c = &l.c;
    let cell = |t: &str| {
        text(t.to_owned())
            .text_size(10.5)
            .weight(600)
            .color(c.muted_foreground.0)
    };
    row()
        .h(22.0)
        .gap(8.0)
        .pad_x(6.0)
        .align(Align::Center)
        .fill(super::style::tones(l).band)
        .radius(super::style::RADIUS)
        .child(cell(titles[0]).w(0.0).grow(1.0))
        .child(cell(titles[1]).w(56.0))
        .child(cell(titles[2]).w(56.0))
        .child(cell(titles[3]).w(56.0))
}

/// A right-aligned number cell.
fn num(l: &ara_kit::look::Look, value: String) -> El<Studio> {
    row().w(56.0).justify(Justify::End).child(
        text(value)
            .text_size(11.5)
            .family(mono())
            .color(l.c.foreground.0),
    )
}

/// A stable colour per scope name from the theme's chart palette.
fn hue(l: &ara_kit::look::Look, name: &str) -> Rgba {
    let c = &l.c;
    let palette = [
        c.chart_1.0,
        c.chart_2.0,
        c.chart_3.0,
        c.chart_4.0,
        c.chart_5.0,
        c.chart_6.0,
    ];
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    palette[h as usize % palette.len()].with_alpha(0.85)
}

/// Nanoseconds as milliseconds.
fn ms(ns: u64) -> String {
    format!("{:.2} ms", ns as f64 / 1e6)
}
