//! Ara's program marks (`assets/brand/icons`): the engine's for the editor, the Hub's for
//! the Hub. Each is rasterized once per pixel size and shared: `brand::mark(Mark::Engine, 60)`.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use stillkit::Image;

/// A program's mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    /// The engine and its editor.
    Engine,
    /// The Hub.
    Hub,
}

impl Mark {
    /// The small-size SVG, authored for compact native chrome.
    pub fn source(self) -> &'static str {
        match self {
            Mark::Engine => include_str!("../../../assets/brand/icons/engine/ara-engine-small.svg"),
            Mark::Hub => include_str!("../../../assets/brand/icons/hub/ara-hub-small.svg"),
        }
    }
}

/// Marks made so far, by mark and pixel size.
type Made = Mutex<HashMap<(Mark, u32), Arc<Image>>>;

/// `mark` rasterized `px` device pixels square (ask for three times its design size, so it
/// is sharp on any display). `None` if the SVG doesn't parse.
pub fn mark(mark: Mark, px: u32) -> Option<Arc<Image>> {
    static MADE: OnceLock<Made> = OnceLock::new();
    let made = MADE.get_or_init(Default::default);
    let mut made = made
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(image) = made.get(&(mark, px)) {
        return Some(image.clone());
    }
    let image = Arc::new(crate::art::svg(mark.source(), px, px)?);
    made.insert((mark, px), image.clone());
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_marks_rasterize_and_are_shared() {
        for m in [Mark::Engine, Mark::Hub] {
            let a = mark(m, 48).expect("the mark parses");
            assert_eq!((a.width, a.height), (48, 48));
            assert!(a.pixels.chunks(4).any(|p| p[3] > 0), "it draws something");
            assert!(Arc::ptr_eq(&a, &mark(m, 48).expect("cached")), "made once");
        }
    }
}
