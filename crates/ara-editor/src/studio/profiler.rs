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
use stillkit::el::{Align, Axis, El, Justify, col, el, icon, row, text};

use super::Studio;

/// Frames the strip shows (the newest).
const STRIP: usize = 120;
/// One nesting level's height in a lane.
const DEPTH: f32 = 15.0;
/// Scopes shorter than this share of the frame are left out of the lanes.
const MIN_SHARE: f64 = 0.002;

/// Asks the player being profiled for `action` (start, stop, a snapshot of its frames).
fn request(s: &mut Studio, action: crate::play_profiling::Action) -> ara_ui::Result<()> {
    let target = s
        .local
        .profiler_target
        .clone()
        .ok_or("Pick a Play player to profile")?;
    let session = s.editor.session.clone();
    let backend = s
        .editor
        .play_profiling
        .as_mut()
        .ok_or("Player profiling is not available in this editor")?;
    let ticket = backend
        .request(crate::play_profiling::Request {
            session,
            target,
            action,
            why: Some("Editor Profiler".into()),
        })
        .map_err(|d| {
            d.first()
                .map_or("The player refused".to_owned(), |d| d.message.clone())
        })?;
    s.local.profiler_ticket = Some((ticket, action));
    Ok(())
}

/// Starts or stops recording what the Profiler profiles: the editor (`profile.start`), or
/// the picked player.
pub(super) fn toggle_record(s: &mut Studio) {
    use crate::play_profiling::Action;
    let result = match (&s.local.profiler_target, s.local.profiler_player_recording) {
        (None, _) if s.local.profiling => s
            .editor
            .action("profile.stop", json!({}))
            .map(drop)
            .map_err(Into::into),
        (None, _) => {
            s.local.profile_frame = None;
            s.editor
                .action("profile.start", json!({}))
                .map(drop)
                .map_err(Into::into)
        }
        (Some(_), true) => request(s, Action::Stop),
        (Some(_), false) => {
            s.local.profile_frame = None;
            request(s, Action::Start)
        }
    };
    if let Err(error) = result {
        s.editor.report_failure(&*anyhow::anyhow!("{error}"));
    }
}

/// Follows player profiling while the Profiler shows (`visible`): lists the players once a
/// second, reads a recording player's frames four times a second, and takes each answer;
/// whether the panel changed.
pub(super) fn poll(s: &mut Studio, visible: bool) -> bool {
    use std::time::{Duration, Instant};
    let mut changed = false;
    if let Some((ticket, _)) = s.local.profiler_ticket
        && let Some(status) = s
            .editor
            .play_profiling
            .as_mut()
            .and_then(|b| b.poll(ticket))
        && status.finished
    {
        s.local.profiler_ticket = None;
        s.local.profiler_player_recording = status.recording;
        if let Some(profile) = status.profile {
            s.profile = std::sync::Arc::new(profile);
        }
        if let Some(d) = status.diagnostics.first().filter(|_| {
            matches!(
                status.outcome,
                Some(
                    crate::play_profiling::Outcome::Failed
                        | crate::play_profiling::Outcome::Refused
                )
            )
        }) {
            s.editor.report_failure(&*anyhow::anyhow!("{}", d.message));
        }
        changed = true;
    }
    if !visible && !s.local.profiler_player_recording {
        return changed;
    }
    let now = Instant::now();
    let (listed, read) = s
        .local
        .profiler_polled
        .unwrap_or((now - Duration::from_secs(2), now));
    let mut polled = (listed, read);
    if now.duration_since(listed) >= Duration::from_secs(1)
        && let Some(backend) = s.editor.play_profiling.as_mut()
    {
        let before = backend.targets().len();
        let _ = backend.refresh_targets();
        changed |= backend.targets().len() != before;
        polled.0 = now;
        // A player that left (Play stopped) is no longer the target.
        if let Some(target) = &s.local.profiler_target
            && !backend.targets().iter().any(|r| &r.target == target)
        {
            s.local.profiler_target = None;
            s.local.profiler_player_recording = false;
            changed = true;
        }
    }
    if s.local.profiler_player_recording
        && s.local.profiler_ticket.is_none()
        && now.duration_since(read) >= Duration::from_millis(250)
    {
        let _ = request(s, crate::play_profiling::Action::Snapshot);
        polled.1 = now;
    }
    s.local.profiler_polled = Some(polled);
    changed
}

/// What the Profiler records, to pick from: the editor, or a running player.
fn target_menu(s: &Studio) -> El<Studio> {
    let l = &s.look;
    const KEY: &str = "profiler/target";
    let open = s.local.select.as_deref() == Some(KEY);
    let rows: Vec<crate::play_profiling::Row> = s
        .editor
        .play_profiling
        .as_ref()
        .map(|b| b.targets().to_vec())
        .unwrap_or_default();
    let title = match &s.local.profiler_target {
        None => "Editor".to_owned(),
        Some(target) => rows
            .iter()
            .find(|r| &r.target == target)
            .map_or_else(|| format!("Player {}", target.player), |r| r.name.clone()),
    };
    let mut trigger = select(l, Size::Sm, KEY, &title)
        .w(190.0)
        .expanded(open)
        .on_press(|s: &mut Studio| {
            s.local.select = match s.local.select.as_deref() == Some(KEY) {
                true => None,
                false => Some(KEY.into()),
            }
        });
    if open {
        let mut items = vec![
            menu_check(
                l,
                &format!("{KEY}/editor"),
                "Editor",
                s.local.profiler_target.is_none(),
            )
            .on_press(|s: &mut Studio| {
                s.local.select = None;
                s.local.profiler_target = None;
                s.local.profile_frame = None;
                s.profile = std::sync::Arc::new(s.editor.host.profiler().profile().clone());
            }),
        ];
        if rows.is_empty() {
            items.push(
                row().pad_x(8.0).h(26.0).align(Align::Center).child(
                    text("Start Play to profile the game")
                        .text_size(11.5)
                        .color(l.c.muted_foreground.0),
                ),
            );
        }
        for r in rows {
            let on = s.local.profiler_target.as_ref() == Some(&r.target);
            let target = r.target.clone();
            items.push(enabled(
                menu_check(l, &format!("{KEY}/{}", r.target.player), &r.name, on)
                    .tooltip(r.reason.clone().unwrap_or_else(|| r.target.label()))
                    .on_press(move |s: &mut Studio| {
                        s.local.select = None;
                        s.local.profiler_target = Some(target.clone());
                        s.local.profiler_player_recording = false;
                        s.local.profile_frame = None;
                    }),
                r.available,
            ));
        }
        trigger = trigger.overlay(
            stillkit::el::menu(menu_panel(l, items).min_w(220.0))
                .on_dismiss(|s: &mut Studio| s.local.select = None),
        );
    }
    trigger
}

/// The panel.
pub(super) fn view(s: &Studio) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let profile = &*s.profile;
    // Recording: the editor's own capture, or the picked player's.
    let recording = match s.local.profiler_target {
        None => s.local.profiling,
        Some(_) => s.local.profiler_player_recording,
    };
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
        .child(target_menu(s))
        .child(baseline_controls(s))
        .child(if recording {
            button(l, Tone::Danger, Size::Sm, Some(icons::CIRCLE_STOP), "Stop")
                .key("profiler/stop")
                .on_press(toggle_record)
        } else {
            button(
                l,
                Tone::Secondary,
                Size::Sm,
                Some(icons::CIRCLE_DOT),
                "Record",
            )
            .key("profiler/record")
            .on_press(toggle_record)
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
        // Its menu: Record, Freeze, Set Baseline, Compare.
        .on_context_menu(|s: &mut Studio, x, y| {
            s.local.context = Some((
                crate::catalog::menus::Context::Profiler,
                crate::catalog::Target::None,
                (x, y),
            ));
        })
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
                        // Within its bar, inset from its ends: a long name ends early
                        // rather than running on.
                        row()
                            .w(w)
                            .h(DEPTH - 1.0)
                            .pad_x(4.0)
                            .align(Align::Center)
                            .child(
                                text(label.clone())
                                    .text_size(10.0)
                                    .color(ink(tint))
                                    .w(0.0)
                                    .grow(1.0)
                                    .wrap(1),
                            )
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
    let tree = s.local.profiler_tree;
    let switch = row()
        .gap(2.0)
        .align(Align::Center)
        .child(
            toggle_button(l, Size::Sm, icons::LIST, "Flat: each scope's totals", !tree)
                .key("profiler/flat")
                .on_press(|s: &mut Studio| s.local.profiler_tree = false),
        )
        .child(
            toggle_button(
                l,
                Size::Sm,
                icons::LIST_TREE,
                "Tree: scopes under their callers",
                tree,
            )
            .key("profiler/tree")
            .on_press(|s: &mut Studio| s.local.profiler_tree = true),
        );
    let table = match tree {
        true => call_tree(s, profile, frame),
        false => flat(s, profile, frame),
    };
    col().gap(4.0).child(switch).child(table)
}

/// One node of the Profiler's call tree: a scope name under its caller's path.
#[derive(Default)]
struct Node {
    name: String,
    depth: usize,
    calls: u32,
    total: u64,
    own: u64,
    children: Vec<String>,
}

/// The frame's spans as Unity's Hierarchy view has them: each scope under the scope that
/// called it (same-named siblings merged), heaviest first, folding by path.
fn call_tree(s: &Studio, profile: &Profile, frame: &Frame) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let name = |label: u32| {
        profile
            .labels
            .get(label as usize)
            .cloned()
            .unwrap_or_default()
    };
    let by_id: std::collections::HashMap<u64, &ara_profile::CpuSpan> =
        frame.spans.iter().map(|span| (span.id, span)).collect();
    // Each span's path of names from its root, memoised by id.
    let mut paths: std::collections::HashMap<u64, String> = Default::default();
    fn path_of(
        id: u64,
        by_id: &std::collections::HashMap<u64, &ara_profile::CpuSpan>,
        paths: &mut std::collections::HashMap<u64, String>,
        name: &dyn Fn(u32) -> String,
        depth: usize,
    ) -> String {
        if let Some(path) = paths.get(&id) {
            return path.clone();
        }
        let Some(span) = by_id.get(&id) else {
            return String::new();
        };
        let own = name(span.label);
        let path = match span.parent {
            // A parent that is missing (or a cycle past any real depth) starts a root.
            parent if parent != 0 && by_id.contains_key(&parent) && depth < 64 => {
                format!("{}/{own}", path_of(parent, by_id, paths, name, depth + 1))
            }
            _ => own,
        };
        paths.insert(id, path.clone());
        path
    }
    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    let mut roots: Vec<String> = Vec::new();
    for span in &frame.spans {
        let path = path_of(span.id, &by_id, &mut paths, &name, 0);
        let parent = path.rsplit_once('/').map(|(p, _)| p.to_owned());
        let node = nodes.entry(path.clone()).or_insert_with(|| Node {
            name: name(span.label),
            depth: path.matches('/').count(),
            ..Node::default()
        });
        node.calls += 1;
        node.total += span.duration_ns;
        node.own += span.self_ns;
        match parent {
            Some(parent) => {
                let up = nodes.entry(parent).or_default();
                if !up.children.contains(&path) {
                    up.children.push(path);
                }
            }
            None if !roots.contains(&path) => roots.push(path),
            None => {}
        }
    }
    let total =
        |path: &String, nodes: &BTreeMap<String, Node>| nodes.get(path).map_or(0, |n| n.total);
    roots.sort_by_key(|p| std::cmp::Reverse(total(p, &nodes)));
    let mut table = col()
        .key("profiler/calls")
        .gap(1.0)
        .child(header(l, &["Scope", "Calls", "Total", "Self"]));
    // Depth first, children of open rows only.
    let mut stack: Vec<String> = roots.into_iter().rev().collect();
    let mut shown = 0;
    while let Some(path) = stack.pop() {
        let Some(node) = nodes.get(&path) else {
            continue;
        };
        shown += 1;
        if shown > 200 {
            break;
        }
        let open = s.local.profiler_open.contains(&path);
        let parent = !node.children.is_empty();
        let toggle = path.clone();
        table = table.child(
            row()
                .key(format!("profiler/call/{path}"))
                .h(22.0)
                .gap(6.0)
                .pad_x(6.0)
                .align(Align::Center)
                .on_press(move |s: &mut Studio| {
                    if !s.local.profiler_open.remove(&toggle) {
                        s.local.profiler_open.insert(toggle.clone());
                    }
                })
                .child(el().w(node.depth as f32 * 12.0))
                .child(match parent {
                    true => icon(if open {
                        icons::CHEVRON_DOWN
                    } else {
                        icons::CHEVRON_RIGHT
                    })
                    .size(11.0)
                    .color(c.muted_foreground.0),
                    false => el().size(11.0),
                })
                .child(el().size(8.0).radius(2.0).fill(hue(l, &node.name)))
                .child(
                    text(node.name.clone())
                        .text_size(12.0)
                        .color(c.foreground.0)
                        .w(0.0)
                        .grow(1.0)
                        .wrap(1),
                )
                .child(num(l, node.calls.to_string()))
                .child(num(l, ms(node.total)))
                .child(num(l, ms(node.own))),
        );
        if open {
            let mut children = node.children.clone();
            children.sort_by_key(|p| total(p, &nodes));
            stack.extend(children);
        }
    }
    table
}

/// The frame's scopes merged by name, most self time first, against a baseline if set.
fn flat(s: &Studio, profile: &Profile, frame: &Frame) -> El<Studio> {
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
    let mut table = col().gap(1.0).child(header(
        l,
        if s.editor.profile_baseline.is_some() && !s.local.profiler_hide_compare {
            &["Scope", "Calls", "Total", "Self", "vs base"]
        } else {
            &["Scope", "Calls", "Total", "Self"]
        },
    ));
    // With a baseline: each scope's mean self time now against the baseline's.
    let compare = s
        .editor
        .profile_baseline
        .as_ref()
        .filter(|_| !s.local.profiler_hide_compare)
        .map(|base| (means(profile), means(base)));
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
                .child(num(l, ms(own)))
                .children(compare.as_ref().map(|(now, base)| {
                    let name = profile
                        .labels
                        .get(label as usize)
                        .cloned()
                        .unwrap_or_default();
                    let (a, b) = (
                        now.get(&name).copied().unwrap_or(0.0),
                        base.get(&name).copied(),
                    );
                    match b {
                        Some(b) => {
                            let delta = a - b;
                            // Slower by more than 5% reads as a warning, faster as success.
                            let tint = if delta > b * 0.05 && delta > 10_000.0 {
                                c.destructive_foreground.0
                            } else if -delta > b * 0.05 && -delta > 10_000.0 {
                                c.success_foreground.0
                            } else {
                                c.muted_foreground.0
                            };
                            text(format!(
                                "{}{:.2}",
                                if delta >= 0.0 { "+" } else { "−" },
                                delta.abs() / 1e6
                            ))
                            .text_size(11.5)
                            .family(mono())
                            .color(tint)
                            .w(56.0)
                        }
                        None => text("new")
                            .text_size(11.5)
                            .color(c.info_foreground.0)
                            .w(56.0),
                    }
                })),
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
        .child(header(l, &["GPU pass", "", "", "Time"]));
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

fn header(l: &ara_kit::look::Look, titles: &[&str]) -> El<Studio> {
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
        .children(titles[1..].iter().map(|t| cell(t).w(56.0)))
}

/// Each scope's mean self time per frame across `profile`'s frames, by scope name (two
/// captures number their labels apart).
fn means(profile: &Profile) -> std::collections::HashMap<String, f64> {
    let mut out = std::collections::HashMap::new();
    let frames = profile.frames.len().max(1) as f64;
    for frame in &profile.frames {
        for span in &frame.spans {
            let name = profile
                .labels
                .get(span.label as usize)
                .cloned()
                .unwrap_or_default();
            *out.entry(name).or_insert(0.0) += span.self_ns as f64 / frames;
        }
    }
    out
}

/// The baseline in the toolbar: Set Baseline freezes what the Profiler shows to compare
/// later captures with (Unity's Profile Analyzer); once set, its frame count and Clear.
fn baseline_controls(s: &Studio) -> El<Studio> {
    let l = &s.look;
    match &s.editor.profile_baseline {
        None => enabled(
            button(l, Tone::Ghost, Size::Sm, Some(icons::FLAG), "Set baseline")
                .key("profiler/baseline")
                .tooltip("Compare later captures' scopes with this one")
                .on_press(|s: &mut Studio| {
                    s.editor.profile_baseline = Some(s.profile.clone());
                }),
            !s.profile.frames.is_empty(),
        ),
        Some(base) => row()
            .gap(4.0)
            .align(Align::Center)
            .child(chip(
                l,
                Status::Info,
                &format!("Baseline · {} frames", base.frames.len()),
                false,
            ))
            .child(
                icon_button(l, Tone::Ghost, Size::Sm, icons::X, "Clear the baseline")
                    .key("profiler/baseline/clear")
                    .on_press(|s: &mut Studio| s.editor.profile_baseline = None),
            ),
    }
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

/// Text over a bar of `tint`: dark on light tints, light on dark ones.
fn ink(tint: Rgba) -> Rgba {
    let luma = 0.2126 * tint.r + 0.7152 * tint.g + 0.0722 * tint.b;
    match luma > 0.42 {
        true => Rgba::new(0.0, 0.0, 0.0, 0.85),
        false => Rgba::new(1.0, 1.0, 1.0, 0.92),
    }
}
