//! Renderer-local readiness; external asset owners must also settle publication.
use crate::Presenter;
use serde::Serialize;

/// Cheap, read-only readiness after a completed presentation. No I/O or waiting.
/// External AssetSource/import work belongs to its owner and must be checked
/// separately for live, unpacked projects.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PresentationReadiness {
    /// A renderer is installed and has the content it draws (it loads its own content
    /// inside its blocking frames).
    pub renderer: bool,
    /// At least one completed image has been published.
    pub published: bool,
    /// The last frame adopted a replacement generation; wait for a stable frame.
    pub content_rebuilt: bool,
    /// The last presentation failed; `Presenter::presentation_error` names it.
    pub failed: bool,
}

impl PresentationReadiness {
    /// Everything the presenter tracks has settled. Continue presenting until this
    /// holds, then apply the caller's configured warmup.
    pub fn is_ready(self) -> bool {
        self.renderer && self.published && !self.failed && !self.content_rebuilt
    }
}

impl Presenter {
    /// Readiness with reasons for a benchmark/loading timeout. Call after `present`;
    /// this never waits and does not treat moving actors as pending.
    pub fn readiness(&self) -> PresentationReadiness {
        PresentationReadiness {
            renderer: self.external.is_some(),
            published: self.published.is_some(),
            content_rebuilt: self.stats.rebuilt,
            failed: self.failure.is_some(),
        }
    }
}

