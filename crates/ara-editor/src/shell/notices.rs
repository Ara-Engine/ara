//! Non-blocking notices over the Scene view (D24): what an agent is looking at, with
//! View (jump once) and Follow, and overwrite reports. Nothing here moves the human's
//! camera or selection by itself. While following an agent a chip says so, with Stop.
use super::commands::Target;
use super::*;
use crate::activity::{NoticeKind, short_author};

impl Shell {
    pub(super) fn notices_markup(&mut self, editor: &Editor, out: &mut String) {
        if let Some(agent) = editor.state.follow.clone() {
            let name = editor
                .state
                .presence
                .get(&agent)
                .map(|p| short_author(&p.author))
                .unwrap_or_else(|| "agent".into());
            self.bind_command("following-stop", "agents.stop_following", Target::None);
            out.push_str(&format!("<panel id='following' class='following'>{}<text id='following-text' text='Following {}' /><button id='following-stop' class='notice-action' label='Stop following' description='Return the Scene view to your own camera' action='Dispatch'><text id='following-stop-label' text='Stop' /></button></panel>",icons::icon_in("following","bot","accent"),escape(&name)));
        }
        let notices = editor.activity.notices();
        if notices.is_empty() {
            return;
        }
        out.push_str("<panel id='notices' class='notice-stack'>");
        for notice in notices {
            let id = format!("notice-{}", notice.id);
            let (icon, tint) = match notice.kind {
                NoticeKind::AgentView { .. } => ("bot", "accent"),
                NoticeKind::Overwrite => ("triangle-alert", "warning"),
            };
            out.push_str(&format!(
                "<panel id='{id}' class='notice'>{}<text id='{id}-text' class='notice-text' text='{}' />",
                icons::icon_in(&id, icon, tint),
                escape(&notice.text)
            ));
            if let NoticeKind::AgentView { agent } = &notice.kind {
                let target = Target::Text(agent.clone());
                self.bind_command(&format!("{id}-view"), "agents.view", target.clone());
                self.bind_command(&format!("{id}-follow"), "agents.follow", target);
                out.push_str(&format!("<button id='{id}-view' class='notice-action' label='View' description='Jump to what the agent sees, once' action='Dispatch'><text id='{id}-view-label' text='View' /></button><button id='{id}-follow' class='notice-action' label='Follow' description='Keep your view on the agent until you stop' action='Dispatch'><text id='{id}-follow-label' text='Follow' /></button>"));
            }
            self.bind_command(
                &format!("{id}-close"),
                "notice.dismiss",
                Target::Text(notice.id.to_string()),
            );
            out.push_str(&format!("<button id='{id}-close' class='ghost icon-button' label='Dismiss' description='Dismiss' action='Dispatch'>{}</button></panel>",icons::icon(&format!("{id}-close"),"x")));
        }
        out.push_str("</panel>");
    }
}
