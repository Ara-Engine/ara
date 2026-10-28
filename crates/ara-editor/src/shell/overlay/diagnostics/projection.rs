//! Homogeneous frustum clipping prevents near-plane flips and unbounded UI coordinates.
use super::*;
use glam::{Vec2, Vec4};
pub(super) fn project(
    wires: &[Segment],
    out: &mut Vec<Line>,
    counts: &mut Counts,
    camera: &Camera,
    rect: Box2,
) {
    if ![
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        rect.x + rect.w,
        rect.y + rect.h,
    ]
    .into_iter()
    .all(f32::is_finite)
        || rect.w <= 0.
        || rect.h <= 0.
    {
        counts.skipped_lines = wires.len();
        return;
    }
    let Ok(camera) = camera
        .spec()
        .resolve([rect.w.max(1.) as u32, rect.h.max(1.) as u32])
    else {
        counts.skipped_lines = wires.len();
        return;
    };
    let matrix = camera.projection * camera.view;
    for wire in wires {
        let Some([from, to]) = clip(matrix * wire.from.extend(1.), matrix * wire.to.extend(1.))
        else {
            counts.skipped_lines += 1;
            continue;
        };
        let screen = |point: Vec4| {
            let ndc = point.truncate() / point.w;
            [
                (rect.x + (ndc.x + 1.) * 0.5 * rect.w).clamp(rect.x, rect.x + rect.w),
                (rect.y + (1. - ndc.y) * 0.5 * rect.h).clamp(rect.y, rect.y + rect.h),
            ]
        };
        let (from, to) = (screen(from), screen(to));
        if !from.into_iter().chain(to).all(f32::is_finite)
            || Vec2::from(from).distance_squared(Vec2::from(to)) < 0.01
        {
            counts.skipped_lines += 1;
            continue;
        }
        out.push(Line {
            from,
            to,
            tint: wire.tint,
        });
    }
}
fn clip(from: Vec4, to: Vec4) -> Option<[Vec4; 2]> {
    if !from.is_finite() || !to.is_finite() {
        return None;
    }
    let distances = |p: Vec4| [p.w + p.x, p.w - p.x, p.w + p.y, p.w - p.y, p.z, p.w - p.z];
    let (a, b) = (distances(from), distances(to));
    let (mut first, mut last) = (0_f32, 1_f32);
    for (a, b) in a.into_iter().zip(b) {
        if a < 0. && b < 0. {
            return None;
        }
        if (a < 0.) != (b < 0.) {
            let at = a / (a - b);
            if a < 0. {
                first = first.max(at)
            } else {
                last = last.min(at)
            }
        }
    }
    if first > last {
        return None;
    }
    let clipped = [from.lerp(to, first), from.lerp(to, last)];
    clipped
        .iter()
        .all(|point| point.is_finite() && point.w > 1e-6)
        .then_some(clipped)
}
