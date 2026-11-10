//! Diagnostics for what the Renderlab path does not render: nothing is dropped
//! silently. Each feature reports once per renderer (`ARA_RENDERLAB_UNSUPPORTED`), each exceeded limit
//! once (`ARA_RENDERLAB_LIMIT`).
use std::collections::BTreeSet;

/// Features and limits already reported.
#[derive(Default)]
pub(crate) struct Gaps {
    reported: BTreeSet<&'static str>,
}

impl Gaps {
    /// A feature of this frame that Renderlab does not render; `effect` says what
    /// the frame shows instead.
    pub fn unsupported(&mut self, feature: &'static str, effect: &str) {
        if self.reported.insert(feature) {
            eprintln!(
                "ARA_RENDERLAB_UNSUPPORTED: the Renderlab renderer path does not support {feature}: {effect}"
            );
        }
    }

    /// A Renderlab limit this frame exceeds; `effect` says what was left out.
    pub fn limit(&mut self, limit: &'static str, effect: &str) {
        if self.reported.insert(limit) {
            eprintln!("ARA_RENDERLAB_LIMIT: the scene exceeds Renderlab's {limit}: {effect}");
        }
    }

    /// Whether `feature` was reported (tests).
    #[cfg(test)]
    pub fn reported(&self, feature: &str) -> bool {
        self.reported.contains(feature)
    }
}
