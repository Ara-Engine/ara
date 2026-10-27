//! Typed drag gestures shared by native surfaces. Hosts resolve targets and commit commands.
use crate::paint::Box2;

/// Movement in logical desktop pixels before a press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 6.;

/// A pointer expressed in both its current surface and one common desktop coordinate space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    /// Stable surface identity; zero conventionally identifies the main window.
    pub surface: u64,
    /// Logical coordinates in the current surface, used for hit testing and previews.
    pub local: [f32; 2],
    /// Logical desktop coordinates, used to keep the threshold stable across windows.
    pub desktop: [f32; 2],
}
impl Pointer {
    fn finite(self) -> bool {
        self.local
            .into_iter()
            .chain(self.desktop)
            .all(f32::is_finite)
    }
}

/// Relationship to a row under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Insert immediately before the row.
    Before,
    /// Insert into the row's container.
    Inside,
    /// Insert immediately after the row.
    After,
}

/// The shape painted for an accepted or rejected destination.
#[derive(Clone, Copy, Debug)]
pub enum Indicator {
    /// A thin insertion caret between rows.
    Between(Box2),
    /// A highlighted target container or field.
    Inside(Box2),
}
impl Indicator {
    /// Bounds in the destination surface's logical coordinates.
    pub fn rect(self) -> Box2 {
        match self {
            Self::Between(rect) | Self::Inside(rect) => rect,
        }
    }
}

/// A resolved destination. Rejections remain visible without mutating the model.
#[derive(Clone, Debug)]
pub enum Feedback<T> {
    /// A valid destination, lowered by the host to a typed command on release.
    Accept {
        /// Domain-specific stable target identity.
        target: T,
        /// Insertion or containment geometry.
        indicator: Indicator,
        /// Human-readable action shown beside the drag ghost.
        label: String,
    },
    /// A target that cannot accept this payload.
    Reject {
        /// Explanation exposed visually and through accessibility.
        reason: String,
        /// Destination geometry, when the pointer is over a known target.
        indicator: Option<Indicator>,
    },
}

/// The only successful outcome of a release; cancellation and invalid drops produce no commit.
#[derive(Debug)]
pub struct Drop<P, T> {
    /// Owned source payload captured at the beginning of the gesture.
    pub payload: P,
    /// Validated destination identity.
    pub target: T,
    /// Final pointer, including the destination surface identity.
    pub pointer: Pointer,
}

/// A single press-to-release gesture. Payload and target types never erase to JSON.
#[derive(Debug)]
pub struct DragSession<P, T> {
    payload: P,
    origin: Pointer,
    pointer: Pointer,
    active: bool,
    cancelled: bool,
    feedback: Option<Feedback<T>>,
}
impl<P, T> DragSession<P, T> {
    /// Capture a source press; malformed coordinates cannot start a gesture.
    pub fn begin(payload: P, pointer: Pointer) -> Option<Self> {
        pointer.finite().then_some(Self {
            payload,
            origin: pointer,
            pointer,
            active: false,
            cancelled: false,
            feedback: None,
        })
    }
    /// Source identity and data, borrowed until the gesture ends.
    pub fn payload(&self) -> &P {
        &self.payload
    }
    /// Current pointer in the destination surface.
    pub fn pointer(&self) -> Pointer {
        self.pointer
    }
    /// Translate the desktop coordinate origin once a host obtains native window geometry.
    /// Both endpoints move together, preserving travelled distance and the activation threshold.
    pub fn translate_desktop(&mut self, delta: [f32; 2]) {
        if !delta.into_iter().all(f32::is_finite) {
            self.cancel();
            return;
        }
        for (axis, delta) in delta.into_iter().enumerate() {
            self.origin.desktop[axis] += delta;
            self.pointer.desktop[axis] += delta;
        }
        if !self.origin.finite() || !self.pointer.finite() {
            self.cancel();
        }
    }
    /// Whether movement has crossed the threshold and the gesture can preview a destination.
    pub fn active(&self) -> bool {
        self.active && !self.cancelled
    }
    /// Update pointer capture. Every move invalidates the old target so stale drops cannot commit.
    pub fn move_to(&mut self, pointer: Pointer) -> bool {
        self.feedback = None;
        if !pointer.finite() {
            self.cancel();
            return false;
        }
        self.pointer = pointer;
        self.active |= (pointer.desktop[0] - self.origin.desktop[0])
            .hypot(pointer.desktop[1] - self.origin.desktop[1])
            >= DRAG_THRESHOLD;
        self.active()
    }
    /// Publish one host-validated target. A press below the threshold cannot show a drop affordance.
    pub fn preview(&mut self, feedback: Feedback<T>) {
        if self.active() {
            self.feedback = Some(feedback);
        }
    }
    /// Current accepted or rejected target; the host uses the same value for paint and release.
    pub fn feedback(&self) -> Option<&Feedback<T>> {
        self.active().then_some(self.feedback.as_ref()).flatten()
    }
    /// End a gesture without a command, including Escape, undo, source deletion or lost capture.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.feedback = None;
    }
    /// Consume an accepted release. The host must revalidate domain references before committing.
    pub fn finish(self) -> Option<Drop<P, T>> {
        if !self.active() {
            return None;
        }
        let Feedback::Accept { target, .. } = self.feedback? else {
            return None;
        };
        Some(Drop {
            payload: self.payload,
            target,
            pointer: self.pointer,
        })
    }
}

/// Resolve row thirds as before/inside/after; flat lists use two equal insertion halves.
pub fn row_placement(rect: Box2, point: [f32; 2], inside: bool) -> Option<Placement> {
    if !valid_rect(rect)
        || !point.into_iter().all(f32::is_finite)
        || !rect.contains(point[0], point[1])
    {
        return None;
    }
    let fraction = (point[1] - rect.y) / rect.h;
    Some(if fraction < if inside { 0.25 } else { 0.5 } {
        Placement::Before
    } else if inside && fraction <= 0.75 {
        Placement::Inside
    } else {
        Placement::After
    })
}

/// Paint geometry corresponding exactly to a row relationship.
pub fn row_indicator(rect: Box2, placement: Placement) -> Indicator {
    match placement {
        Placement::Inside => Indicator::Inside(rect),
        Placement::Before | Placement::After => Indicator::Between(Box2 {
            y: rect.y
                + if placement == Placement::After {
                    rect.h
                } else {
                    0.
                }
                - 1.,
            h: 2.,
            ..rect
        }),
    }
}

/// Keep a ghost inside its current viewport, including small detached windows.
pub fn ghost_rect(point: [f32; 2], size: [f32; 2], viewport: Box2) -> Option<Box2> {
    if !valid_rect(viewport) || !point.into_iter().chain(size).all(f32::is_finite) {
        return None;
    }
    let w = size[0].clamp(0., viewport.w);
    let h = size[1].clamp(0., viewport.h);
    Some(Box2 {
        x: (point[0] + 14.).clamp(viewport.x, viewport.x + viewport.w - w),
        y: (point[1] + 18.).clamp(viewport.y, viewport.y + viewport.h - h),
        w,
        h,
    })
}

/// Bounded per-frame edge scrolling, in logical pixels. Hosts call this while capture is held,
/// including frames with no pointer movement; hidden/outside scroll views receive no movement.
pub fn auto_scroll(viewport: Box2, point: [f32; 2], seconds: f32) -> [f32; 2] {
    if !valid_rect(viewport)
        || !point.into_iter().all(f32::is_finite)
        || !seconds.is_finite()
        || seconds <= 0.
        || !viewport.contains(point[0], point[1])
    {
        return [0.; 2];
    }
    let axis = |position: f32, start: f32, extent: f32| {
        let band = 24_f32.min(extent * 0.25);
        let before = (1. - (position - start) / band).clamp(0., 1.);
        let after = (1. - (start + extent - position) / band).clamp(0., 1.);
        (after - before) * 600. * seconds.min(0.1)
    };
    [
        axis(point[0], viewport.x, viewport.w),
        axis(point[1], viewport.y, viewport.h),
    ]
}

fn valid_rect(rect: Box2) -> bool {
    [rect.x, rect.y, rect.w, rect.h]
        .into_iter()
        .all(f32::is_finite)
        && rect.w > 0.
        && rect.h > 0.
}

#[cfg(test)]
#[path = "drag_drop/tests.rs"]
mod tests;
