//! The curve editor, inline where a value is a curve (VFX Size over Life, an Animator
//! transition's blend), as Unity's and Niagara's are: the curve drawn over a light grid,
//! its keys to drag, the picked key's tangent handles, double-click to add a key; below it
//! the picked key's tangent and interpolation modes, time and value, and starting shapes.
//! A drag edits a copy (`Local::curve_drag`) and commits once on release (one undo step).
use ara_curve::{Curve, Interpolation, Preset, Tangent};
use ara_kit::icons;
use ara_kit::ui::*;
use stillkit::el::{Align, El, PointerPhase, col, el, paint, row, text};
use stillkit::{Op, Point, Rect, Rgba};

use super::{Studio, style};

/// What a press on the plot took hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
    /// Key `i`, to move.
    Key(usize),
    /// Key `i`'s arriving tangent handle.
    In(usize),
    /// Key `i`'s leaving tangent handle.
    Out(usize),
}

/// A key or tangent handle being dragged: the field's key, the curve as the drag has it,
/// what was grabbed, and the frame it is drawn in (held still while dragging).
#[derive(Clone, Debug)]
pub struct CurveDrag {
    /// The editor's key (`curve/{field}`).
    pub field: String,
    /// The curve with the drag applied so far.
    pub curve: Curve,
    /// What the press took hold of.
    pub grab: Grab,
    /// The time and value ranges the plot shows.
    pub frame: Frame,
}

/// The time and value ranges a plot shows, and how its pixels map to them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Time at the plot's left and right.
    pub time: (f32, f32),
    /// Value at the plot's bottom and top.
    pub value: (f32, f32),
}

/// Room inside the plot's edges so end keys and handles stay in reach (design px).
const INSET: f32 = 10.0;
/// The plot's height (design px).
pub(super) const PLOT_H: f32 = 132.0;
/// How long a tangent handle is drawn (design px).
const HANDLE: f32 = 36.0;
/// How near a press must be to grab a key or handle (design px).
const REACH: f32 = 8.0;
/// The tangent modes by name, as the editor lists them.
const TANGENTS: [(&str, Tangent); 4] = [
    ("Auto", Tangent::Auto),
    ("Flat", Tangent::Flat),
    ("Free", Tangent::Free),
    ("Broken", Tangent::Broken),
];
/// The segment shapes by name.
const SHAPES: [(&str, Interpolation); 3] = [
    ("Cubic", Interpolation::Cubic),
    ("Linear", Interpolation::Linear),
    ("Constant", Interpolation::Constant),
];

impl Frame {
    /// The frame that shows `curve`: `time` when the field fixes it, else its keys' span;
    /// its values with a margin.
    pub fn of(curve: &Curve, time: Option<(f32, f32)>) -> Self {
        let time = time.or_else(|| curve.span()).map_or((0.0, 1.0), |(a, b)| {
            if b - a > 1e-6 {
                (a, b)
            } else {
                (a - 0.5, a + 0.5)
            }
        });
        let (lo, hi) = curve.value_range(24).unwrap_or((0.0, 1.0));
        let pad = ((hi - lo) * 0.15).max(0.05 * hi.abs().max(lo.abs()).max(1.0));
        Self {
            time,
            value: (lo - pad, hi + pad),
        }
    }

    /// Where `(time, value)` is in a plot of `size` (design px from its top-left).
    pub fn place(&self, (t, v): (f32, f32), (w, h): (f32, f32)) -> (f32, f32) {
        let (iw, ih) = ((w - 2.0 * INSET).max(1.0), (h - 2.0 * INSET).max(1.0));
        (
            INSET + (t - self.time.0) / (self.time.1 - self.time.0) * iw,
            INSET + (self.value.1 - v) / (self.value.1 - self.value.0) * ih,
        )
    }

    /// The `(time, value)` at plot point `(x, y)`.
    pub fn read(&self, (x, y): (f32, f32), (w, h): (f32, f32)) -> (f32, f32) {
        let (iw, ih) = ((w - 2.0 * INSET).max(1.0), (h - 2.0 * INSET).max(1.0));
        (
            self.time.0 + (x - INSET) / iw * (self.time.1 - self.time.0),
            self.value.1 - (y - INSET) / ih * (self.value.1 - self.value.0),
        )
    }

    /// Key `i`'s tangent handles (in, out) in a plot of `size`: a fixed length along each
    /// side's slope as drawn.
    fn handles(&self, curve: &Curve, i: usize, size: (f32, f32)) -> ((f32, f32), (f32, f32)) {
        let k = &curve.keys[i];
        let (x, y) = self.place((k.time, k.value), size);
        let (slope_in, slope_out) = drawn_slopes(curve, i);
        let along = |slope: f32, sign: f32| {
            let (ax, ay) = self.place((k.time + 1.0, k.value + slope), size);
            let (dx, dy) = (ax - x, ay - y);
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            (x + sign * dx / len * HANDLE, y + sign * dy / len * HANDLE)
        };
        (along(slope_in, -1.0), along(slope_out, 1.0))
    }

    /// What a press at `at` takes hold of: the picked key's handles first, then any key.
    fn grab(
        &self,
        curve: &Curve,
        picked: Option<usize>,
        at: (f32, f32),
        size: (f32, f32),
    ) -> Option<Grab> {
        let near = |p: (f32, f32)| (p.0 - at.0).hypot(p.1 - at.1) <= REACH;
        if let Some(i) = picked.filter(|&i| i < curve.keys.len()) {
            let (h_in, h_out) = self.handles(curve, i, size);
            if near(h_out) && i + 1 < curve.keys.len() {
                return Some(Grab::Out(i));
            }
            if near(h_in) && i > 0 {
                return Some(Grab::In(i));
            }
        }
        curve
            .keys
            .iter()
            .position(|k| near(self.place((k.time, k.value), size)))
            .map(Grab::Key)
    }
}

/// Key `i`'s slopes as the curve is drawn on each side: a cubic side's own, a straight
/// side along its line, a constant side level.
fn drawn_slopes(curve: &Curve, i: usize) -> (f32, f32) {
    let (own_in, own_out) = curve.slopes(i);
    let side = |a: usize, own: f32| {
        let (k, next) = (&curve.keys[a], curve.keys.get(a + 1));
        match (k.interpolation, next) {
            (Interpolation::Linear, Some(n)) => (n.value - k.value) / (n.time - k.time),
            (Interpolation::Constant, Some(_)) => 0.0,
            _ => own,
        }
    };
    let arriving = i.checked_sub(1).map_or(own_in, |p| side(p, own_in));
    (arriving, side(i, own_out))
}

/// Applies a drag to `(time, value)` (curve units) to `curve`: a key moves, kept in order and
/// within `time`; a handle bends its segment, setting that side's slope (both sides unless
/// the key is Broken).
pub fn drag(curve: &mut Curve, grab: Grab, (t, v): (f32, f32), time: Option<(f32, f32)>) {
    match grab {
        Grab::Key(i) => {
            let t = time.map_or(t, |(a, b)| t.clamp(a, b));
            curve.move_key(i, t, v);
        }
        Grab::In(i) | Grab::Out(i) => {
            let out = matches!(grab, Grab::Out(_));
            // A handle bends its segment: that segment becomes cubic.
            let segment = if out { Some(i) } else { i.checked_sub(1) };
            if let Some(s) = segment {
                curve.keys[s].interpolation = Interpolation::Cubic;
            }
            let k = &mut curve.keys[i];
            // Keep the handle on its own side of the key.
            let dt = match out {
                true => (t - k.time).max(1e-4),
                false => (t - k.time).min(-1e-4),
            };
            let slope = (v - k.value) / dt;
            match (k.tangent, out) {
                (Tangent::Broken, true) => k.out_slope = slope,
                (Tangent::Broken, false) => k.in_slope = slope,
                _ => {
                    k.tangent = Tangent::Free;
                    k.out_slope = slope;
                    k.in_slope = slope;
                }
            }
        }
    }
}

/// `preset` over `curve`'s time span and value range, so a size curve stays in metres.
fn shaped(preset: Preset, curve: &Curve, time: Option<(f32, f32)>) -> Curve {
    let (t0, t1) = time.or_else(|| curve.span()).unwrap_or((0.0, 1.0));
    let (lo, hi) = match curve.value_range(8) {
        Some((lo, hi)) if hi - lo > 1e-6 => (lo, hi),
        _ => (0.0, 1.0),
    };
    let mut out = preset.curve();
    for k in &mut out.keys {
        k.time = t0 + k.time * (t1 - t0);
        k.value = lo + k.value * (hi - lo);
        let scale = (hi - lo) / (t1 - t0).max(1e-6);
        k.in_slope *= scale;
        k.out_slope *= scale;
    }
    out.before = curve.before;
    out.after = curve.after;
    out
}

/// The editor for `curve` under `name` (unique on the page); `time` fixes its time range
/// (keys stay inside it), `min_keys` how few keys it may have. `commit` saves a change.
pub(super) fn editor(
    s: &Studio,
    name: &str,
    curve: &Curve,
    time: Option<(f32, f32)>,
    min_keys: usize,
    commit: impl Fn(&mut Studio, Curve) + Clone + 'static,
) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let t = style::tones(l);
    let key = format!("curve/{name}");
    let live = s.local.curve_drag.as_ref().filter(|d| d.field == key);
    let shown = live.map_or(curve, |d| &d.curve).clone();
    let frame = live.map_or_else(|| Frame::of(&shown, time), |d| d.frame);
    let picked = s
        .local
        .curve_pick
        .as_ref()
        .filter(|(f, i)| *f == key && *i < shown.keys.len())
        .map(|(_, i)| *i);
    let ink = (
        c.primary.0,
        c.foreground.0,
        c.muted_foreground.0.with_alpha(0.16),
        c.muted_foreground.0.with_alpha(0.35),
    );
    let drawn = shown.clone();
    let plot = el()
        .key(key.clone())
        .w_full()
        .h(PLOT_H)
        .radius(style::RADIUS)
        .fill(t.well)
        .label("Curve: drag keys and handles, double-click to add a key")
        .on_pointer({
            // The saved curve: what a press grabs from, and what a release compares with
            // (the view during a drag shows the drag's copy).
            let (key, curve, commit) = (key.clone(), curve.clone(), commit.clone());
            move |s: &mut Studio, e: &stillkit::el::PointerEvent, _: &mut stillkit::el::Tx| {
                if e.button != stillkit::Button::Left {
                    return false;
                }
                let at = (e.local.x, e.local.y);
                match e.phase {
                    PointerPhase::Down => {
                        let picked = s
                            .local
                            .curve_pick
                            .as_ref()
                            .filter(|(f, _)| *f == key)
                            .map(|(_, i)| *i);
                        match frame.grab(&curve, picked, at, e.size) {
                            Some(grab) => {
                                let (Grab::Key(i) | Grab::In(i) | Grab::Out(i)) = grab;
                                s.local.curve_pick = Some((key.clone(), i));
                                s.local.curve_drag = Some(CurveDrag {
                                    field: key.clone(),
                                    curve: curve.clone(),
                                    grab,
                                    frame,
                                });
                            }
                            None if e.clicks >= 2 => {
                                let (mut t, v) = frame.read(at, e.size);
                                if let Some((a, b)) = time {
                                    t = t.clamp(a, b);
                                }
                                let mut next = curve.clone();
                                let i = next.insert(t, v);
                                s.local.curve_pick = Some((key.clone(), i));
                                commit(s, next);
                            }
                            None => s.local.curve_pick = None,
                        }
                    }
                    PointerPhase::Move => {
                        let Some(d) = s.local.curve_drag.as_mut().filter(|d| d.field == key) else {
                            return false;
                        };
                        let point = d.frame.read(at, e.size);
                        drag(&mut d.curve, d.grab, point, time);
                    }
                    PointerPhase::Up => {
                        if let Some(d) = s.local.curve_drag.take().filter(|d| d.field == key)
                            && d.curve != curve
                        {
                            commit(s, d.curve);
                        }
                    }
                    PointerPhase::Cancel => s.local.curve_drag = None,
                    PointerPhase::Hover => return false,
                }
                true
            }
        })
        .child(
            paint(move |f, rect, _| draw(f, rect, &drawn, frame, picked, ink))
                .w_full()
                .h_full(),
        );
    let mut tools = row().gap(4.0).align(Align::Center);
    tools = tools.child(super::animator::dropdown(
        s,
        format!("{key}/shape"),
        "Shape",
        Preset::ALL.iter().map(|p| p.name().to_owned()).collect(),
        {
            let (curve, commit) = (curve.clone(), commit.clone());
            move |s: &mut Studio, name: &str| {
                if let Some(preset) = Preset::ALL.into_iter().find(|p| p.name() == name) {
                    s.local.curve_pick = None;
                    commit(s, shaped(preset, &curve, time));
                }
            }
        },
    ));
    let hint = || {
        text("Double-click to add a key")
            .text_size(11.0)
            .color(c.muted_foreground.0)
    };
    let Some(i) = picked else {
        return col().gap(6.0).child(plot).child(tools.child(hint()));
    };
    let k = shown.keys[i];
    let edit = |change: fn(&mut ara_curve::Key, &str) -> bool| {
        let (curve, commit) = (curve.clone(), commit.clone());
        move |s: &mut Studio, picked: &str| {
            let mut next = curve.clone();
            if let Some(k) = next.keys.get_mut(i)
                && change(k, picked)
            {
                commit(s, next);
            }
        }
    };
    fn name_of<T: PartialEq>(pairs: &[(&'static str, T)], v: &T) -> &'static str {
        pairs.iter().find(|(_, m)| m == v).map_or("", |(n, _)| *n)
    }
    tools = tools
        .child(super::animator::dropdown(
            s,
            format!("{key}/tangent"),
            name_of(&TANGENTS, &k.tangent),
            TANGENTS.iter().map(|(n, _)| (*n).to_owned()).collect(),
            edit(|k, picked| {
                TANGENTS
                    .iter()
                    .find(|(n, _)| *n == picked)
                    .map(|(_, mode)| k.tangent = *mode)
                    .is_some()
            }),
        ))
        .child(super::animator::dropdown(
            s,
            format!("{key}/interpolation"),
            name_of(&SHAPES, &k.interpolation),
            SHAPES.iter().map(|(n, _)| (*n).to_owned()).collect(),
            edit(|k, picked| {
                SHAPES
                    .iter()
                    .find(|(n, _)| *n == picked)
                    .map(|(_, mode)| k.interpolation = *mode)
                    .is_some()
            }),
        ));
    let number = |label: &str, slot: &str, value: f32, set: fn(&mut Curve, usize, f32)| {
        let (curve, commit) = (curve.clone(), commit.clone());
        row()
            .w(0.0)
            .grow(1.0)
            .gap(4.0)
            .align(Align::Center)
            .child(
                text(label.to_owned())
                    .text_size(11.0)
                    .color(c.muted_foreground.0),
            )
            .child(
                field(
                    l,
                    Size::Sm,
                    format!("{key}/{slot}"),
                    None,
                    "",
                    &format!("{}", (value * 1000.0).round() / 1000.0),
                    move |x| {
                        x.select_all_on_focus()
                            .on_submit(move |s: &mut Studio, typed| {
                                if let Ok(v) = typed.trim().parse::<f32>()
                                    && v.is_finite()
                                {
                                    let mut next = curve.clone();
                                    set(&mut next, i, v);
                                    // A fixed time range keeps the key inside it.
                                    if let Some((a, b)) = time {
                                        let k = next.keys[i];
                                        next.move_key(i, k.time.clamp(a, b), k.value);
                                    }
                                    commit(s, next);
                                }
                            })
                    },
                )
                .w(0.0)
                .grow(1.0),
            )
    };
    let keys = shown.keys.len();
    let remove = {
        let (curve, commit) = (curve.clone(), commit.clone());
        move |s: &mut Studio| {
            let mut next = curve.clone();
            next.keys.remove(i);
            s.local.curve_pick = None;
            commit(s, next);
        }
    };
    let values = row()
        .gap(8.0)
        .align(Align::Center)
        .child(number("Time", "time", k.time, |c: &mut Curve, i, v| {
            c.move_key(i, v, c.keys[i].value);
        }))
        .child(number("Value", "value", k.value, |c: &mut Curve, i, v| {
            c.keys[i].value = v;
        }))
        .child(enabled(
            icon_button(l, Tone::Ghost, Size::Sm, icons::TRASH, "Delete key")
                .size(22.0)
                .key(format!("{key}/delete"))
                .on_press(remove),
            keys > min_keys,
        ));
    col().gap(6.0).child(plot).child(tools).child(values)
}

/// Paints the grid, the curve across the whole plot (its extrapolation too), the keys,
/// and the picked key's tangent handles.
fn draw(
    f: &mut stillkit::Frame,
    rect: Rect,
    curve: &Curve,
    frame: Frame,
    picked: Option<usize>,
    (accent, key_ink, grid, handle_ink): (Rgba, Rgba, Rgba, Rgba),
) {
    let k = f.px(1.0);
    let size = (rect.width() / k, rect.height() / k);
    let at = |(x, y): (f32, f32)| Point::new(rect.left + x * k, rect.top + y * k);
    let line = |f: &mut stillkit::Frame, a: (f32, f32), b: (f32, f32), width: f32, color: Rgba| {
        f.push(Op::Line {
            from: at(a),
            to: at(b),
            width: width * k,
            color,
        });
    };
    // Quarters of the time and value ranges.
    for q in 0..=4 {
        let u = q as f32 / 4.0;
        let x = INSET + u * (size.0 - 2.0 * INSET);
        let y = INSET + u * (size.1 - 2.0 * INSET);
        line(f, (x, INSET), (x, size.1 - INSET), 1.0, grid);
        line(f, (INSET, y), (size.0 - INSET, y), 1.0, grid);
    }
    if curve.keys.is_empty() {
        return;
    }
    let clamp_y = |y: f32| y.clamp(1.0, size.1 - 1.0);
    let steps = (size.0 / 3.0).max(8.0) as usize;
    let mut last = None;
    for s in 0..=steps {
        let x = s as f32 / steps as f32 * size.0;
        let (t, _) = frame.read((x, 0.0), size);
        let (_, y) = frame.place((t, curve.evaluate(t)), size);
        let p = (x, clamp_y(y));
        if let Some(prev) = last {
            line(f, prev, p, 1.75, accent);
        }
        last = Some(p);
    }
    if let Some(i) = picked {
        let (h_in, h_out) = frame.handles(curve, i, size);
        let key = frame.place((curve.keys[i].time, curve.keys[i].value), size);
        let sides = [(h_in, i > 0), (h_out, i + 1 < curve.keys.len())];
        for (h, shown) in sides {
            if shown {
                line(f, key, h, 1.0, handle_ink);
                let r = 3.5 * k;
                let c = at(h);
                f.fill(Rect::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r), r, key_ink);
            }
        }
    }
    for (i, key) in curve.keys.iter().enumerate() {
        let (x, y) = frame.place((key.time, key.value), size);
        let c = at((x, clamp_y(y)));
        let on = picked == Some(i);
        let r = if on { 5.0 } else { 4.0 } * k;
        f.fill(
            Rect::new(c.x - r, c.y - r, 2.0 * r, 2.0 * r),
            r,
            if on { accent } else { key_ink },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_points_map_both_ways_and_presses_find_keys_and_handles() {
        let curve = Curve::linear(&[(0.0, 0.0), (1.0, 2.0)]);
        let frame = Frame::of(&curve, Some((0.0, 1.0)));
        let size = (200.0, 120.0);
        let p = frame.place((0.5, 1.0), size);
        let back = frame.read(p, size);
        assert!((back.0 - 0.5).abs() < 1e-5 && (back.1 - 1.0).abs() < 1e-5);
        let end = frame.place((1.0, 2.0), size);
        assert_eq!(frame.grab(&curve, None, end, size), Some(Grab::Key(1)));
        let (_, out) = frame.handles(&curve, 0, size);
        assert_eq!(frame.grab(&curve, Some(0), out, size), Some(Grab::Out(0)));
        assert_eq!(frame.grab(&curve, None, (100.0, 5.0), size), None);
    }

    #[test]
    fn dragging_a_handle_frees_the_key_unless_it_is_broken() {
        let mut curve = Curve::linear(&[(0.0, 0.0), (1.0, 1.0)]);
        drag(&mut curve, Grab::Out(0), (0.5, 1.0), Some((0.0, 1.0)));
        assert_eq!(curve.keys[0].tangent, Tangent::Free);
        assert!((curve.keys[0].out_slope - 2.0).abs() < 1e-5);
        curve.keys[1].tangent = Tangent::Broken;
        drag(&mut curve, Grab::In(1), (0.5, 0.0), None);
        assert!((curve.keys[1].in_slope - 2.0).abs() < 1e-5);
        assert_eq!(curve.keys[1].out_slope, 0.0, "the other side stays");
        drag(&mut curve, Grab::Key(1), (3.0, 0.5), Some((0.0, 1.0)));
        assert_eq!(curve.keys[1].time, 1.0, "kept within the fixed time range");
    }

    #[test]
    fn a_shape_keeps_the_curve_s_range() {
        let size = Curve::linear(&[(0.0, 0.2), (1.0, 1.4)]);
        let eased = shaped(Preset::EaseIn, &size, Some((0.0, 1.0)));
        assert!((eased.evaluate(0.0) - 0.2).abs() < 1e-5);
        assert!((eased.evaluate(1.0) - 1.4).abs() < 1e-5);
        assert!(
            (eased.evaluate(0.5) - (0.2 + 1.2 * 0.25)).abs() < 1e-4,
            "still eases in"
        );
    }
}
