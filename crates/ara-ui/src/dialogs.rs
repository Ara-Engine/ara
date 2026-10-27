//! Host-independent native confirmations, prompts and alerts.
//!
//! Append [`Dialogs::markup`] under the UI root, include [`CSS`] in the stylesheet,
//! call [`Dialogs::sync`] after reconcile/frame and route input through
//! [`Dialogs::event`] before host shortcuts. Requests return typed futures without
//! blocking the editor, Hub or game loop. Content mutations remain host commands.
mod completion;
use crate::{
    Result, Ui,
    input::{InputEvent, InputState},
    source::{escape, stable_id},
};
pub use completion::DialogFuture;
use completion::Reply;

/// Shared semantic styles; the host supplies theme tokens, including `--overlay`.
pub const CSS: &str = include_str!("dialogs/dialogs.css");

/// Copy and dismissal policy for a confirmation, prompt or alert.
#[derive(Clone, Debug)]
pub struct DialogOptions {
    /// Short title announced with the dialog role.
    pub title: String,
    /// Wrapping explanatory text.
    pub message: String,
    /// Explicit action label, such as Delete or Save.
    pub accept_label: String,
    /// Label for the safe cancellation action.
    pub cancel_label: String,
    /// Style the primary action destructively and initially focus Cancel.
    pub destructive: bool,
    /// Whether a press on the backdrop cancels; false by default.
    pub close_on_backdrop: bool,
}

impl DialogOptions {
    /// A dialog with explicit text and safe defaults; hosts can customize its labels.
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            accept_label: "OK".into(),
            cancel_label: "Cancel".into(),
            destructive: false,
            close_on_backdrop: false,
        }
    }
}

enum Response {
    Confirm(Reply<bool>),
    Prompt(Reply<Option<String>>),
    Alert(Reply<()>),
}

struct Request {
    id: String,
    options: DialogOptions,
    text: Option<String>,
    response: Option<Response>,
}

impl Request {
    fn finish(&mut self, accepted: bool) {
        match self.response.take() {
            Some(Response::Confirm(reply)) => reply.finish(accepted),
            Some(Response::Prompt(reply)) => {
                reply.finish(accepted.then(|| self.text.take().unwrap_or_default()))
            }
            Some(Response::Alert(reply)) => reply.finish(()),
            None => {}
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.finish(false);
    }
}

/// A per-surface modal stack. Requests and completions are UI state only; a result
/// authorizes the host to dispatch its typed operation through its command registry.
pub struct Dialogs {
    prefix: String,
    next: u64,
    requests: Vec<Request>,
    mounted: Vec<String>,
}

impl Default for Dialogs {
    fn default() -> Self {
        Self::new("ara-dialog")
    }
}

impl Dialogs {
    /// Give a controller a unique markup prefix when several share one UI surface.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            next: 0,
            requests: Vec::new(),
            mounted: Vec::new(),
        }
    }

    /// Ask for explicit confirmation; Esc, Cancel and controller drop resolve false.
    pub fn confirm(&mut self, options: DialogOptions) -> DialogFuture<bool> {
        let (reply, future) = Reply::new();
        self.push(options, None, Response::Confirm(reply));
        future
    }

    /// Ask for a single-line value; cancellation resolves None, preserving an empty
    /// accepted string as Some(""). Validation and authoring belong to the host.
    pub fn prompt(
        &mut self,
        options: DialogOptions,
        initial: impl Into<String>,
    ) -> DialogFuture<Option<String>> {
        let (reply, future) = Reply::new();
        self.push(options, Some(initial.into()), Response::Prompt(reply));
        future
    }

    /// Show an informational/error alert; either dismissal resolves its future.
    pub fn alert(&mut self, options: DialogOptions) -> DialogFuture<()> {
        let (reply, future) = Reply::new();
        self.push(options, None, Response::Alert(reply));
        future
    }

    fn push(&mut self, options: DialogOptions, text: Option<String>, response: Response) {
        self.next = self.next.wrapping_add(1);
        self.requests.push(Request {
            id: format!("{}-{}", self.prefix, self.next),
            options,
            text,
            response: Some(response),
        });
    }

    /// Whether input should be routed to this controller before host shortcuts.
    pub fn is_open(&self) -> bool {
        !self.requests.is_empty()
    }

    /// Number of stacked requests, including the currently interactive top one.
    pub fn len(&self) -> usize {
        self.requests.len()
    }

    /// Whether there are no pending requests.
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    /// Render all stacked requests as root children. Logical viewport dimensions
    /// keep dialogs bounded at every DPI; text and body content wrap/scroll.
    pub fn markup(&self, viewport: [f32; 2]) -> String {
        let mut out = String::new();
        let [w, h] = viewport.map(|v| if v.is_finite() { v.max(1.0) } else { 1.0 });
        let width = crate::popover::MAX_WIDTH.min((w - 16.0).max(1.0));
        let height = (h - 16.0).max(1.0);
        for request in &self.requests {
            let id = escape(&request.id, b'\'');
            let options = &request.options;
            let esc = |s: &str| escape(s, b'\'');
            out.push_str(&format!("<modal id='{id}' class='kit-dialog-layer' label='{}' default-action='{id}-accept'><panel id='{id}-backdrop' class='kit-dialog-backdrop' /><panel id='{id}-body' class='kit-dialog' style='width:{width}px;max-height:{height}px'><text id='{id}-title' class='kit-dialog-title' text='{}' /><scroll id='{id}-scroll' class='kit-dialog-body'><text id='{id}-message' class='kit-dialog-message' text='{}' /></scroll>", esc(&options.title), esc(&options.title), esc(&options.message)));
            if let Some(text) = &request.text {
                out.push_str(&format!(
                    "<field id='{id}-field' text='{}' label='{}' action='ara.dialog.text' />",
                    esc(text),
                    esc(&options.title)
                ));
            }
            out.push_str(&format!(
                "<panel id='{id}-actions' class='kit-dialog-actions'>"
            ));
            if !matches!(&request.response, Some(Response::Alert(_))) {
                out.push_str(&format!("<button id='{id}-cancel' class='kit-dialog-button' action='ara.dialog.cancel' text='{}' />", esc(&options.cancel_label)));
            }
            out.push_str(&format!("<button id='{id}-accept' class='kit-dialog-button {}' action='ara.dialog.accept' text='{}' /></panel></panel></modal>", if options.destructive { "kit-dialog-destructive" } else { "kit-dialog-accept" }, esc(&options.accept_label)));
        }
        out
    }

    /// Reconcile focus scopes after mounting and laying out markup. Existing modal
    /// identities survive full host rebuilds; nested scopes restore their own focus.
    pub fn sync(&mut self, ui: &mut Ui, input: &mut InputState) -> Result<()> {
        let common = self
            .mounted
            .iter()
            .zip(&self.requests)
            .take_while(|(id, request)| **id == request.id)
            .count();
        while self.mounted.len() > common {
            if let Some(id) = self.mounted.pop() {
                input.remove_scope(ui, stable_id(&id));
            }
        }
        for request in &self.requests[common..] {
            let initial = format!(
                "{}-{}",
                request.id,
                if request.options.destructive {
                    "cancel"
                } else if request.text.is_some() {
                    "field"
                } else {
                    "accept"
                }
            );
            input.push(ui, stable_id(&request.id), Some(stable_id(&initial)), true)?;
            self.mounted.push(request.id.clone());
        }
        Ok(())
    }

    /// Temporarily unmount modal scopes before the host replaces an underlying
    /// screen scope. Requests remain pending; sync remounts them over the new root.
    pub fn suspend(&mut self, ui: &mut Ui, input: &mut InputState) {
        while let Some(id) = self.mounted.pop() {
            input.remove_scope(ui, stable_id(&id));
        }
    }

    /// Consume modal input before the host's shortcuts. Returns true while a modal
    /// owns input; no pointer, wheel or navigation event reaches underlying panels.
    pub fn event(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        event: InputEvent,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> Result<bool> {
        if self.requests.is_empty() {
            return Ok(false);
        }
        self.sync(ui, input)?;
        if matches!(event, InputEvent::Cancel) && input.scrubbing() {
            input.event(ui, event, clipboard)?;
            return Ok(true);
        }
        if matches!(event, InputEvent::Cancel) {
            self.resolve(ui, input, false)?;
            return Ok(true);
        }
        let request = self
            .requests
            .last()
            .ok_or("ARA4046: dialog stack changed during input")?;
        if let InputEvent::PointerDown(point) = &event
            && ui
                .find(&format!("{}-body", request.id))
                .is_some_and(|i| !ui.nodes[i].rect.contains(point[0], point[1]))
        {
            if request.options.close_on_backdrop {
                self.resolve(ui, input, false)?;
            }
            return Ok(true);
        }
        if matches!(event, InputEvent::Activate)
            && input
                .focused()
                .and_then(|id| ui.by_id(id))
                .is_none_or(|i| matches!(ui.nodes[i].element.tag.as_str(), "field" | "input"))
        {
            self.resolve(ui, input, true)?;
            return Ok(true);
        }
        input.event(ui, event, clipboard)?;
        self.dispatch(ui, input)?;
        Ok(true)
    }

    /// Consume only this controller's actions; unrelated actions remain queued for
    /// the host. Call after routing UI input externally instead of through event.
    pub fn dispatch(&mut self, ui: &mut Ui, input: &mut InputState) -> Result<()> {
        let actions = std::mem::take(&mut ui.actions);
        for action in actions {
            let own = self.requests.last().is_some_and(|r| {
                ["accept", "cancel", "field"]
                    .iter()
                    .any(|suffix| action.node == stable_id(&format!("{}-{suffix}", r.id)))
            });
            if !own {
                ui.actions.push(action);
                continue;
            }
            match action.name.as_str() {
                "ara.dialog.accept" => self.resolve(ui, input, true)?,
                "ara.dialog.cancel" => self.resolve(ui, input, false)?,
                "ara.dialog.text" => {
                    if let Some(text) = action.value.as_str()
                        && let Some(request) = self.requests.last_mut()
                    {
                        request.text = Some(text.to_owned());
                    }
                }
                _ => ui.actions.push(action),
            }
        }
        Ok(())
    }

    fn resolve(&mut self, ui: &mut Ui, input: &mut InputState, accepted: bool) -> Result<()> {
        if let Some(mut request) = self.requests.pop() {
            if let Some(index) = ui.find(&request.id) {
                ui.set_visible(index, false)?;
            }
            request.finish(accepted);
        }
        self.sync(ui, input)
    }
}

#[cfg(test)]
mod tests;
