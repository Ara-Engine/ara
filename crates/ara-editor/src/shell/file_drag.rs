//! Native file destinations use the shared typed session; the OS owns its source ghost.
use super::*;
use ara_ui::drag_drop::{self as kit, DragSession, Feedback, Indicator, Pointer};
mod paint;
mod targets;

struct Files {
    batch: u64,
}
/// Accepted destination from the same cached snapshot used by the visible browser.
pub(crate) struct Folder {
    pub batch: u64,
    pub storage: String,
    pub authored: String,
}
#[derive(Default)]
pub(crate) struct State {
    batch: Option<u64>,
    session: Option<DragSession<Files, Folder>>,
}

fn surface(shell: &Shell) -> u64 {
    shell
        .current
        .as_deref()
        .map_or(0, ara_ui::source::stable_id)
}
impl Shell {
    /// Native BEGIN has no coordinates; a first Position/FileDrop activates its session.
    pub(crate) fn begin_file_drag(&mut self, batch: u64) -> ara_ui::Result<()> {
        self.note_pointer_chord();
        self.cancel_file_drag()?;
        self.file_drag.batch = Some(batch);
        self.invalidate_file_drag();
        Ok(())
    }
    pub(crate) fn file_drag_pending(&self) -> bool {
        self.file_drag.batch.is_some()
    }
    /// The caller enters the tagged native surface before supplying logical coordinates.
    pub(crate) fn file_drag_over(
        &mut self,
        editor: &Editor,
        local: [f32; 2],
        desktop: [f32; 2],
        seconds: f32,
    ) -> ara_ui::Result<bool> {
        let Some(batch) = self.file_drag.batch else {
            return Ok(false);
        };
        let pointer = Pointer {
            surface: surface(self),
            local,
            desktop,
        };
        let mut state = std::mem::take(&mut self.file_drag);
        let result = (|| -> ara_ui::Result<bool> {
            if state.session.is_none() {
                state.session = DragSession::external(Files { batch }, pointer);
            }
            let Some(session) = &mut state.session else {
                return Err("The native file drag position is invalid".into());
            };
            if !session.move_to(pointer) {
                return Err("The native file drag position is invalid".into());
            }
            self.pointer = local;
            let scroll = !self.modal_active()
                && !self.choice_open()
                && !self.content_pending()
                && !self.tab_pending();
            for id in ["asset-rows", "asset-grid", "asset-folders"]
                .into_iter()
                .filter(|_| scroll)
            {
                let Some(index) = self.shown_node(id) else {
                    continue;
                };
                let rect = self.ui.nodes[index]
                    .rect
                    .intersection(self.ui.nodes[index].clip);
                let delta = kit::auto_scroll(rect, local, seconds);
                if delta != [0.; 2]
                    && let Some(offset) = self.ui.scroll_position(index)
                {
                    self.ui
                        .scroll_to(index, [offset[0] + delta[0], offset[1] + delta[1]])?;
                    self.sync_scroll_views()?;
                }
            }
            session.preview(self.file_drag_target(editor, batch));
            Ok(true)
        })();
        self.file_drag = state;
        self.invalidate_file_drag();
        result
    }
    /// Re-resolve after a retained layout/virtual-pool update, before painting.
    pub(crate) fn refresh_file_drag_feedback(&mut self, editor: &Editor) {
        let Some(batch) = self.file_drag.batch else {
            return;
        };
        let Some(pointer) = self
            .file_drag
            .session
            .as_ref()
            .map(DragSession::pointer)
            .filter(|pointer| pointer.surface == surface(self))
        else {
            return;
        };
        let previous = self.pointer;
        self.pointer = pointer.local;
        let feedback = self.file_drag_target(editor, batch);
        self.pointer = previous;
        if let Some(session) = &mut self.file_drag.session {
            session.preview(feedback);
        }
    }
    /// Finish returns only a typed accepted folder. The app revalidates it before queueing.
    pub(crate) fn finish_file_drag(&mut self) -> ara_ui::Result<Option<Folder>> {
        let state = std::mem::take(&mut self.file_drag);
        self.hide_file_drag_label()?;
        self.invalidate_file_drag();
        Ok(state
            .session
            .and_then(DragSession::finish)
            .filter(|drop| Some(drop.payload.batch) == state.batch)
            .map(|drop| drop.target))
    }
    /// Escape, lost focus and a disappearing destination discard the entire gesture.
    pub(crate) fn cancel_file_drag(&mut self) -> ara_ui::Result<bool> {
        if self.file_drag.batch.take().is_none() {
            return Ok(false);
        }
        if let Some(session) = &mut self.file_drag.session {
            session.cancel();
        }
        self.file_drag.session = None;
        self.hide_file_drag_label()?;
        self.invalidate_file_drag();
        Ok(true)
    }
    pub(crate) fn file_drag_report(&self) -> Value {
        let Some(batch) = self.file_drag.batch else {
            return Value::Null;
        };
        let feedback=self.file_drag.session.as_ref().and_then(DragSession::feedback).map(|feedback|match feedback {
            Feedback::Accept{target,indicator,label}=>json!({"valid":true,"storagePath":target.storage,"authoredPath":target.authored,"rect":indicator.rect(),"label":label}),
            Feedback::Reject{reason,indicator}=>json!({"valid":false,"reason":reason,"rect":indicator.map(Indicator::rect)}),
        });
        json!({"batch":batch,"surface":self.file_drag.session.as_ref().map(|session|session.pointer().surface),"pointer":self.file_drag.session.as_ref().map(|session|session.pointer().local),"feedback":feedback})
    }
}
