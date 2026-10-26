use super::*;

impl Renderer {
    /// Allocated GI still has clears, record writes, initial relighting or its
    /// arrival fade to show. An absent volume is idle, including a failed bake.
    pub fn probe_gi_needs_frame(&self) -> bool {
        !self.look.probe_writes.is_empty()
            || self
                .look
                .gi
                .as_ref()
                .is_some_and(|gi| gi.fresh || gi.arriving())
    }

    /// Whether the acquired GI volume is fully delivered, scattered, converged
    /// and faded into shading. Query after a completed frame; absent GI is false.
    pub fn probe_gi_ready(&self) -> bool {
        self.look.probe_writes.is_empty()
            && self.look.settle == 0
            && self
                .look
                .gi
                .as_ref()
                .is_some_and(|gi| !gi.fresh && gi.received >= gi.probes as usize && !gi.arriving())
    }
}
