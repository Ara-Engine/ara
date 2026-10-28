//! Authority-checked player results and immutable capture publication.
use super::*;

impl Controller {
    pub(super) fn receive(
        &mut self,
        world: &str,
        index: usize,
        source: &str,
        value: Value,
        root: &Path,
        publish: &mut impl FnMut(Diagnostic),
    ) {
        let Ok(reply) = serde_json::from_value::<Reply>(value) else {
            return;
        };
        self.accept(world, index, source, reply, root, publish);
    }

    pub(super) fn decode_failed(
        &mut self,
        world: &str,
        index: usize,
        source: &str,
        ticket: u64,
        root: &Path,
        publish: &mut impl FnMut(Diagnostic),
    ) {
        if let Some(status) = self
            .tasks
            .iter_mut()
            .filter_map(|task| task.status.as_mut())
            .find(|status| {
                status.ticket.0 == ticket
                    && status.target.world == world
                    && status.target.player as usize == index
                    && status.target.source == source
                    && !status.finished
            })
        {
            finish(
                status,
                PlayProfileOutcome::Failed,
                problem(
                    root,
                    "Player profile capture decode queue failed or the capture was invalid",
                ),
                publish,
            );
        }
    }

    pub(super) fn accept(
        &mut self,
        world: &str,
        index: usize,
        source: &str,
        reply: Reply,
        root: &Path,
        publish: &mut impl FnMut(Diagnostic),
    ) {
        if !self.rows.iter().any(|row| {
            row.target.world == world
                && row.target.player as usize == index
                && row.target.source == source
                && row.reason.as_deref() != Some("The selected player incarnation exited")
        }) {
            return;
        }
        if let Some(row) = self.rows.iter_mut().find(|row| {
            row.target.world == world
                && row.target.player as usize == index
                && row.target.source == source
        }) {
            let revision = self.revisions.entry(row.target.clone()).or_default();
            if reply.revision >= *revision {
                *revision = reply.revision;
                if row.recording != reply.recording
                    || row.available != reply.available
                    || row.reason != reply.reason
                {
                    self.dirty = true;
                }
                row.recording = reply.recording;
                row.available = reply.available;
                row.reason = reply.reason;
            }
        }
        if reply.ticket == 0 {
            return;
        }
        let Some(status) = self
            .tasks
            .iter_mut()
            .filter_map(|task| task.status.as_mut())
            .find(|status| {
                status.ticket.0 == reply.ticket
                    && status.target.world == world
                    && status.target.player as usize == index
                    && status.target.source == source
                    && !status.finished
            })
        else {
            return;
        };
        if reply
            .profile
            .as_ref()
            .is_some_and(|profile| profile.version != 1 || profile.frames.len() > 120)
        {
            finish(
                status,
                PlayProfileOutcome::Failed,
                problem(
                    root,
                    "Player profiler returned an unsupported schema or exceeded its 120 completed-frame capacity",
                ),
                publish,
            );
            return;
        }
        for mut diagnostic in reply.diagnostics {
            if diagnostic.location.file.is_none() {
                diagnostic.location.file =
                    Some(root.join("ara.project.json").to_string_lossy().into_owned());
            }
            let diagnostic = crate::scripts::DiagnosticSources::bound(
                world,
                index,
                source,
                status.target.binding,
                diagnostic,
            );
            publish(diagnostic.clone());
            status.diagnostics.push(diagnostic);
        }
        status.recording = self
            .rows
            .iter()
            .find(|row| row.target == status.target)
            .is_some_and(|row| row.recording);
        status.profile = reply.profile;
        status.outcome = Some(reply.outcome.unwrap_or(PlayProfileOutcome::Failed));
        status.finished = true;
    }
}
