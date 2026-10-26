use super::*;

impl Renderer {
    /// Allocated GI still has clears, record writes, initial relighting or its
    /// arrival fade to show, including a pool's later finer lattices. An absent
    /// volume is idle, including a failed bake.
    pub fn probe_gi_needs_frame(&self) -> bool {
        !self.look.probe_writes.is_empty()
            || self
                .look
                .gi
                .as_ref()
                .is_some_and(|gi| gi.fresh || gi.arriving() || gi.deferred_arriving())
    }

    /// Whether the acquired GI volume's coarsest lattice is delivered, scattered,
    /// converged and faded into shading. A pool's finer lattices may land, converge
    /// and cross-fade in afterwards without unsetting this. Query after a
    /// completed frame; absent GI is false.
    pub fn probe_gi_ready(&self) -> bool {
        self.look
            .probe_writes
            .iter()
            .all(|&(_, _, deferred)| deferred)
            && self.look.settle == 0
            && self
                .look
                .gi
                .as_ref()
                .is_some_and(|gi| !gi.fresh && gi.landed() && !gi.arriving())
    }
}
