//! Map only this proof's tagged native motion; never poll the owner's OS pointer.
use ara_platform::Event;

#[derive(Debug)]
pub(super) struct Mapping {
    pub window_rect: [i32; 4],
    pub native: [f32; 2],
    pub logical: [f32; 2],
    pub desktop: [f32; 2],
}

/// Each measured age currently emits at most one distinct pointer position, all
/// tagged to the supplied main window. Fail closed if a future floating/multi-
/// position batch would require a different per-event surface mapping.
pub(super) fn map(
    window: u32,
    window_rect: [i32; 4],
    scale: f32,
    events: &[(u32, Event)],
) -> ara_ui::Result<Option<Mapping>> {
    let mut point = None;
    for (surface, event) in events {
        let Event::MouseMotion { position, .. } = event else {
            continue;
        };
        if *surface != window {
            return Err("Asset proof motion belongs to a different native window; supply its own origin and scale".into());
        }
        if point.is_some_and(|previous| previous != *position) {
            return Err("Asset proof queued multiple pointer positions in one age; route them in separate native ages".into());
        }
        point = Some(*position);
    }
    let Some(native) = point else {
        return Ok(None);
    };
    let logical = native.map(|value| value * scale);
    let desktop = [
        window_rect[0] as f32 + native[0],
        window_rect[1] as f32 + native[1],
    ];
    if !scale.is_finite()
        || scale <= 0.
        || !native
            .iter()
            .chain(&logical)
            .chain(&desktop)
            .all(|value| value.is_finite())
    {
        return Err(
            "Asset proof native pointer mapping is nonfinite or has an invalid scale".into(),
        );
    }
    Ok(Some(Mapping {
        window_rect,
        native,
        logical,
        desktop,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn motion(surface: u32, position: [f32; 2]) -> (u32, Event) {
        (
            surface,
            Event::MouseMotion {
                position,
                delta: [0.; 2],
            },
        )
    }

    #[test]
    fn tagged_sdl_points_keep_desktop_origin_independent_of_logical_scale() {
        let one = map(7, [-1800, 200, 1440, 900], 1., &[motion(7, [775., 777.])])
            .unwrap()
            .unwrap();
        let two = map(
            7,
            [-1800, 200, 1440, 900],
            0.5,
            &[motion(7, [1550., 1554.])],
        )
        .unwrap()
        .unwrap();
        assert_eq!(one.logical, two.logical);
        assert_eq!(one.desktop, [-1025., 977.]);
        assert_eq!(two.desktop, [-250., 1754.]);
    }

    #[test]
    fn ambiguous_or_nonfinite_native_batches_cannot_silently_poll_or_map_another_window() {
        assert!(map(7, [0, 0, 1440, 900], 1., &[motion(8, [1., 2.])]).is_err());
        assert!(
            map(
                7,
                [0, 0, 1440, 900],
                1.,
                &[motion(7, [1., 2.]), motion(7, [3., 4.])]
            )
            .is_err()
        );
        assert!(map(7, [0, 0, 1440, 900], 1., &[motion(7, [f32::NAN, 2.])]).is_err());
        assert!(map(7, [0, 0, 1440, 900], 0., &[motion(7, [1., 2.])]).is_err());
        assert!(map(7, [0, 0, 1440, 900], 1., &[]).unwrap().is_none());
    }
}
