//! Drawing for one transform operation, reused by the universal gizmo.
use super::*;

/// Draw one operation; universal handles reuse the same geometry and hit shapes.
pub(super) fn draw_handles(
    overlays: &mut Vec<Overlay>,
    handles: &Handles,
    gizmo: GizmoState,
    color: &impl Fn(&str, f32) -> ara_ui::Result<[f32; 4]>,
    draw_centre: bool,
) -> ara_ui::Result<()> {
    if !Vec2::from(handles.origin).is_finite() {
        return Ok(());
    }
    let halo = [0., 0., 0., 0.6];
    let hover = color("warning-foreground", 1.)?;
    let tints = [
        color("destructive-foreground", 1.)?,
        color("success-foreground", 1.)?,
        color("info-foreground", 1.)?,
    ];
    let faded = |c: [f32; 4], handle: Handle| -> [f32; 4] {
        match gizmo.active {
            Some(active) if active != handle => [c[0], c[1], c[2], c[3] * 0.15],
            _ if gizmo.hover == Some(handle) || gizmo.active == Some(handle) => hover,
            _ => c,
        }
    };
    let stroke =
        |overlays: &mut Vec<Overlay>, from: [f32; 2], to: [f32; 2], width: f32, c: [f32; 4]| {
            overlays.push(Overlay::Line {
                from,
                to,
                width: width + 1.,
                color: [halo[0], halo[1], halo[2], halo[3] * c[3]],
            });
            overlays.push(Overlay::Line {
                from,
                to,
                width,
                color: c,
            });
        };
    // Opaque fills (cones, cubes) sit on a 1 px dark halo, like the strokes.
    let solid = |overlays: &mut Vec<Overlay>, corners: [[f32; 2]; 4], c: [f32; 4]| {
        overlays.push(Overlay::Quad {
            corners: grow(corners, 1.),
            color: [halo[0], halo[1], halo[2], halo[3] * c[3]],
        });
        overlays.push(Overlay::Quad { corners, color: c });
    };
    let lit = |handle: Handle| gizmo.hover == Some(handle) || gizmo.active == Some(handle);
    let o = Vec2::from(handles.origin);
    // While dragging, a thin constraint line through the pivot along each moving axis.
    let constrained = match gizmo.active {
        Some(Handle::Axis(axis)) if handles.tool != Tool::Rotate => [Some(axis), None],
        Some(Handle::Plane(axis)) => [Some((axis + 1) % 3), Some((axis + 2) % 3)],
        _ => [None, None],
    };
    for axis in constrained.into_iter().flatten() {
        let direction = handles.screen_dir(axis);
        if !direction.is_finite() || direction == Vec2::ZERO {
            continue;
        }
        let d = direction * 2000.;
        let mut line = tints[axis];
        line[3] = 0.5;
        overlays.push(Overlay::Line {
            from: (o - d).into(),
            to: (o + d).into(),
            width: 0.5,
            color: line,
        });
    }
    if handles.tool == Tool::Rotate {
        // The half of each ring facing away from the camera draws at 25 %, so the three
        // rings read as a sphere rather than a tangle.
        let toward = (handles.pivot - handles.camera.position()).normalize_or_zero();
        for (axis, tint) in tints.iter().enumerate() {
            let c = faded(*tint, Handle::Axis(axis));
            let width = if lit(Handle::Axis(axis)) { 1.25 } else { 0.75 };
            for n in 0..48 {
                let Some([from, to]) = handles.ring_segment(axis, n) else {
                    continue;
                };
                let middle = (n as f32 + 0.5) / 48. * std::f32::consts::TAU;
                let behind = (handles.ring_point(axis, middle) - handles.pivot).dot(toward) > 0.;
                let mut c = c;
                if behind && !lit(Handle::Axis(axis)) {
                    c[3] *= 0.25;
                }
                stroke(overlays, from, to, width, c);
            }
        }
        return Ok(());
    }
    if handles.tool == Tool::Translate {
        // Plane squares first, under the arms: 20 % fill (45 % lit), a 1 px edge.
        for (axis, tint) in tints.iter().enumerate() {
            let handle = Handle::Plane(axis);
            let Some(quad) = handles.plane_quad(axis) else {
                continue;
            };
            let c = faded(*tint, handle);
            let mut fill = c;
            fill[3] *= if lit(handle) { 0.45 } else { 0.2 };
            overlays.push(Overlay::Quad {
                corners: quad,
                color: fill,
            });
            for n in 0..4 {
                stroke(overlays, quad[n], quad[(n + 1) % 4], 0.5, c);
            }
        }
    }
    for (axis, tint) in tints.iter().enumerate() {
        let handle = Handle::Axis(axis);
        let c = faded(*tint, handle);
        let width = if lit(handle) { 1.5 } else { 1. };
        let end = Vec2::from(handles.ends[axis]);
        let dir = handles.screen_dir(axis);
        if !end.is_finite() || !dir.is_finite() || dir == Vec2::ZERO || o.distance(end) < 8. {
            continue; // Axes nearly parallel to the view fade out entirely.
        }
        if handles.tool == Tool::Scale {
            // An 8 px cube (10 px lit) at the arm's end.
            stroke(overlays, handles.origin, (end - dir * 4.).into(), width, c);
            solid(overlays, square(end, if lit(handle) { 5. } else { 4. }), c);
        } else {
            // A 12 px solid cone.
            let base = end - dir * 12.;
            let side = Vec2::new(-dir.y, dir.x) * if lit(handle) { 5.5 } else { 4.5 };
            stroke(overlays, handles.origin, base.into(), width, c);
            solid(
                overlays,
                [
                    end.into(),
                    (base + side).into(),
                    (base - side).into(),
                    end.into(),
                ],
                c,
            );
        }
    }
    if !draw_centre {
        return Ok(());
    }
    let centre = if lit(Handle::Centre) {
        hover
    } else {
        color("foreground", 0.75)?
    };
    if handles.tool == Tool::Scale {
        // A 10 px square for uniform scale.
        solid(overlays, square(o, 5.), centre);
    } else {
        // A 10 px screen-space disc for free movement in the view plane.
        stroke(overlays, handles.origin, handles.origin, 5., centre);
    }
    Ok(())
}
