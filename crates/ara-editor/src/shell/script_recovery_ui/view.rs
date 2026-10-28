use super::*;

impl Shell {
    /// Compact optional content for the existing Play/Problems area; no new page or modal.
    pub(crate) fn script_recovery_markup(&mut self, editor: &Editor, out: &mut String) {
        self.bind_command(
            "script-recovery-reload",
            "tools.cs.reload",
            commands::Target::None,
        );
        self.bind_command(
            "script-recovery-console",
            "window.console",
            commands::Target::None,
        );
        let state = &self.script_recovery_ui;
        let reason = self.script_recovery_blocker(editor);
        out.push_str(&format!("<panel id='script-recovery' style='flex-direction:column;height:auto;min-height:0px;max-height:180px;flex-basis:auto;flex-shrink:1;gap:4px;padding:6px'><panel id='script-recovery-actions' class='row' style='height:auto;min-height:24px;flex-shrink:0;flex-wrap:wrap;gap:4px'><button id='script-recovery-reload' style='height:auto;min-height:24px;min-width:0px;white-space:normal' text='Reload scripts' description='{}' disabled='{}' action='Dispatch' /><button id='script-recovery-console' class='ghost' text='Console' description='View full diagnostic history, including managed exception stacks' action='Dispatch' /></panel>",escape(reason.unwrap_or("Reload C# in every player of the current running world; do not restart Play")),reason.is_some()));
        if let Some(reason) = reason {
            out.push_str(&format!(
                "<text id='script-recovery-reason' class='caption wrap-text' text='{}' />",
                escape(reason)
            ));
        }
        out.push_str("<scroll id='script-recovery-scroll' style='height:auto;min-height:0px;max-height:120px;flex-basis:auto;flex-shrink:1'><panel id='script-recovery-body' style='height:auto;flex-direction:column;gap:4px'>");
        for (ticket, pending) in state.pending.iter().take(8) {
            out.push_str(&format!("<panel id='script-recovery-pending-{ticket}' style='height:auto;flex-direction:column;flex-shrink:0;gap:2px'><text id='script-recovery-pending-{ticket}-label' class='caption wrap-text' text='Reload #{ticket} · {} · pending{}' />",escape(&pending.world),if editor.play_world.as_deref()==Some(pending.world.as_str()){""}else{" (previous world; request remains owned)"}));
            if pending.invalid_reply {
                out.push_str(&format!("<text id='script-recovery-pending-{ticket}-invalid' class='caption wrap-text' text='Waiting for this Play session to report its result. An unrelated reply was ignored.' />"));
            }
            if let Some(status) = &pending.latest {
                status_markup(status, out);
            }
            out.push_str("</panel>");
        }
        if state.pending.len() > 8 {
            out.push_str(&format!("<text id='script-recovery-more-pending' class='caption wrap-text' text='{} additional reload requests remain owned and are polled in turn.' />",state.pending.len()-8));
        }
        if let Some((world, diagnostics)) = &state.request_error {
            out.push_str(&format!("<text id='script-recovery-request-error' class='caption wrap-text' text='Reload was not queued for {}' />",escape(world)));
            diagnostic_markup("script-recovery-request", diagnostics, &mut 16, out);
        }
        for status in &state.terminal {
            status_markup(status, out);
        }
        if state.pending.is_empty() && state.terminal.is_empty() && state.request_error.is_none() {
            out.push_str("<text id='script-recovery-empty' class='caption wrap-text' text='Source compilation remains automatic. Manual reload reports each player and module; Unchanged or Refused does not clear runtime faults.' />");
        }
        out.push_str("</panel></scroll></panel>");
    }
}

pub(super) fn status_markup(status: &ScriptRecoveryStatus, out: &mut String) {
    let id = format!("script-recovery-result-{}", status.ticket.0);
    out.push_str(&format!("<panel id='{id}' style='height:auto;flex-direction:column;flex-shrink:0;gap:2px'><text id='{id}-summary' class='caption wrap-text' text='{}' />",escape(&state::summary(status))));
    let mut budget = 16;
    diagnostic_markup(&id, &status.diagnostics, &mut budget, out);
    for (index, player) in status.players.iter().take(16).enumerate() {
        let prefix = format!("{id}-player-{index}");
        let modules = player
            .modules
            .iter()
            .take(16)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("<text id='{prefix}-scope' class='caption wrap-text' text='Player {} · source {} · binding {} · {} · modules: {}{}' />",player.player,escape(&player.source),player.binding,state::outcome(&player.outcome),escape(&modules),if player.modules.len()>16 { " (additional modules retained in Console)" } else { "" }));
        diagnostic_markup(&prefix, &player.diagnostics, &mut budget, out);
    }
    if status.players.len() > 16 {
        out.push_str(&format!("<text id='{id}-more-players' class='caption wrap-text' text='{} additional player results are retained in Console.' />",status.players.len()-16));
    }
    out.push_str("</panel>");
}

fn diagnostic_markup(
    id: &str,
    diagnostics: &[ara_diag::Diagnostic],
    budget: &mut usize,
    out: &mut String,
) {
    // Bound mounted rows while preserving all severities and prioritizing actual
    // errors. Full diagnostics are retained in Status and already in Console.
    let ordered = diagnostics
        .iter()
        .enumerate()
        .filter(|(_, d)| d.is_error())
        .chain(
            diagnostics
                .iter()
                .enumerate()
                .filter(|(_, d)| d.severity == ara_diag::Severity::Warning),
        )
        .chain(
            diagnostics
                .iter()
                .enumerate()
                .filter(|(_, d)| d.severity == ara_diag::Severity::Info),
        );
    let shown = diagnostics.len().min(*budget);
    *budget -= shown;
    for (index, diagnostic) in ordered.take(shown) {
        out.push_str(&format!(
            "<text id='{id}-diagnostic-{index}' class='caption wrap-text' text='{}: {}' />",
            diagnostic.code,
            escape(&diagnostic.message)
        ));
    }
    if diagnostics.len() > shown {
        out.push_str(&format!("<text id='{id}-more' class='caption wrap-text' text='{} additional diagnostics are retained in Console.' />",diagnostics.len()-shown));
    }
}
