//! The studio's UI zoom: a small window shows the interface a little larger, so a laptop-sized
//! window reads as a composed layout rather than a cramped copy of the large one.
//!
//! The runtime's scale (device px per design px) is the platform's scale times this zoom, so
//! everything, text and padding included, steps together, and the layout sees the window as that
//! much smaller (a 1000 px window at 1.1 lays out 909 design px wide). Windows at and above the
//! widest step are not zoomed.
//!
//! The steps are one table, [`STEPS`]; `ARA_UI_ZOOM=1.2` forces a zoom (for trying a step on a
//! machine without resizing, or turning it off with `1`).

/// Window widths in design px (at the platform's own scale) below which the zoom applies, each
/// with its zoom, narrowest first. The first step whose width the window is under wins.
const STEPS: &[(f32, f32)] = &[(1100.0, 1.1)];

/// The environment variable that forces a zoom.
const OVERRIDE: &str = "ARA_UI_ZOOM";

/// The zoom for a window `width` design px wide at the platform's scale.
pub(super) fn for_width(width: f32) -> f32 {
    forced().unwrap_or_else(|| steps(width))
}

fn steps(width: f32) -> f32 {
    STEPS
        .iter()
        .find(|(below, _)| width < *below)
        .map_or(1.0, |(_, zoom)| *zoom)
}

/// `ARA_UI_ZOOM`, when it is a sane zoom.
fn forced() -> Option<f32> {
    std::env::var(OVERRIDE)
        .ok()?
        .parse::<f32>()
        .ok()
        .filter(|zoom| (0.5..=3.0).contains(zoom))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_windows_zoom_and_large_ones_do_not() {
        assert_eq!(steps(1000.0), 1.1);
        assert_eq!(steps(1099.0), 1.1);
        assert_eq!(steps(1100.0), 1.0);
        assert_eq!(steps(1600.0), 1.0);
    }
}
