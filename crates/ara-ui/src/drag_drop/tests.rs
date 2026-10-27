#![allow(clippy::unwrap_used)] // Tests assert fixture operations directly.
use super::*;

fn pointer(surface: u64, local: [f32; 2], desktop: [f32; 2]) -> Pointer {
    Pointer {
        surface,
        local,
        desktop,
    }
}
fn rect() -> Box2 {
    Box2 {
        x: 0.,
        y: 0.,
        w: 200.,
        h: 100.,
    }
}
fn accepted() -> Feedback<&'static str> {
    Feedback::Accept {
        target: "parent",
        indicator: Indicator::Inside(rect()),
        label: "Move inside".into(),
    }
}
#[test]
fn external_drag_is_active_without_synthetic_motion_and_rejects_nonfinite_pointers() {
    let at = pointer(2, [12., 20.], [312., 220.]);
    let mut external = DragSession::external("native files", at).expect("finite native pointer");
    assert!(external.active());
    external.preview(accepted());
    assert_eq!(
        external.finish().expect("accepted native release").pointer,
        at
    );
    let mut invalid = at;
    invalid.local[0] = f32::NAN;
    assert!(DragSession::<(), ()>::external((), invalid).is_none());
    invalid = at;
    invalid.desktop[1] = f32::INFINITY;
    assert!(DragSession::<(), ()>::external((), invalid).is_none());
    let regular = DragSession::<(), ()>::begin((), at).expect("finite press");
    assert!(!regular.active());
}

#[test]
fn press_is_a_click_until_threshold_and_once_active_stays_active() {
    let mut drag = DragSession::begin("child", pointer(1, [50., 20.], [250., 220.])).unwrap();
    drag.preview(accepted());
    assert!(drag.feedback().is_none());
    assert!(!drag.move_to(pointer(1, [55., 20.], [255., 220.])));
    assert!(drag.move_to(pointer(1, [56., 20.], [256., 220.])));
    assert!(drag.move_to(pointer(1, [50., 20.], [250., 220.])));
    drag.preview(accepted());
    let drop = drag.finish().unwrap();
    assert_eq!(drop.payload, "child");
    assert_eq!(drop.target, "parent");
}

#[test]
fn cross_window_threshold_uses_desktop_coordinates_and_moves_clear_stale_targets() {
    let mut drag = DragSession::begin("child", pointer(1, [190., 20.], [390., 220.])).unwrap();
    assert!(!drag.move_to(pointer(2, [0., 20.], [394., 220.])));
    assert!(drag.move_to(pointer(2, [3., 20.], [397., 220.])));
    drag.preview(accepted());
    drag.move_to(pointer(2, [4., 20.], [398., 220.]));
    assert!(drag.finish().is_none());
}

#[test]
fn invalid_drop_escape_and_nonfinite_motion_never_commit() {
    for cancel in [false, true] {
        let mut drag = DragSession::begin("child", pointer(1, [0.; 2], [0.; 2])).unwrap();
        drag.move_to(pointer(1, [10.; 2], [10.; 2]));
        drag.preview(Feedback::<&str>::Reject {
            reason: "Cannot parent to a descendant".into(),
            indicator: None,
        });
        if cancel {
            drag.cancel();
        }
        assert!(drag.finish().is_none());
    }
    let mut drag = DragSession::begin("child", pointer(1, [0.; 2], [0.; 2])).unwrap();
    drag.move_to(pointer(1, [10.; 2], [10.; 2]));
    drag.preview(accepted());
    drag.move_to(pointer(1, [f32::NAN, 0.], [10.; 2]));
    drag.move_to(pointer(1, [20.; 2], [20.; 2]));
    drag.preview(accepted());
    assert!(drag.finish().is_none());
}

#[test]
fn row_regions_and_indicators_match_and_ghost_fits_tiny_windows() {
    assert_eq!(
        row_placement(rect(), [50., 10.], true),
        Some(Placement::Before)
    );
    assert_eq!(
        row_placement(rect(), [50., 50.], true),
        Some(Placement::Inside)
    );
    assert_eq!(
        row_placement(rect(), [50., 90.], true),
        Some(Placement::After)
    );
    assert_eq!(row_indicator(rect(), Placement::After).rect().y, 99.);
    let ghost = ghost_rect([195., 99.], [250., 30.], rect()).unwrap();
    assert_eq!(ghost.w, 200.);
    assert!(ghost.x >= 0. && ghost.x + ghost.w <= 200.);
    assert!(ghost.y >= 0. && ghost.y + ghost.h <= 100.);
}

#[test]
fn auto_scroll_is_directional_bounded_and_rejects_bad_inputs() {
    assert!(auto_scroll(rect(), [50., 1.], 0.016)[1] < 0.);
    assert!(auto_scroll(rect(), [50., 99.], 0.016)[1] > 0.);
    assert_eq!(auto_scroll(rect(), [50., 50.], 0.016), [0.; 2]);
    assert_eq!(auto_scroll(rect(), [50., 101.], 0.016), [0.; 2]);
    assert_eq!(auto_scroll(rect(), [50., 99.], f32::NAN), [0.; 2]);
    assert!(auto_scroll(rect(), [50., 99.], 1000.)[1] <= 60.);
}
