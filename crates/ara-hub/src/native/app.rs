//! The window's retained `Ui`, reconciled from the view-model on change, and the
//! routing of pointer/keyboard input and `ara_ui` actions back to [`Intent`]s.
use super::{images::Thumbnails, input::OsClipboard};
use crate::{
    model::{Hub, Intent, View},
    view::{self, Binding, Ctx, Doc},
};
use ara_ui::{
    Ui, Work,
    input::{InputEvent, InputState},
    source::stable_id,
    theme::EditorTheme,
    widgets::Widget,
};
use ara_ui_text::{EditEvent, TextSystem};
use std::collections::BTreeMap;

/// UI state of the Hub window.
pub struct App {
    /// Retained tree and display list.
    pub ui: Ui,
    /// Focus, hover and capture.
    pub input: InputState,
    /// Fonts and shaping caches.
    pub text: TextSystem,
    /// Preview images.
    pub thumbnails: Thumbnails,
    /// Learn links.
    pub docs: Vec<Doc>,
    /// Logical window size.
    pub size: [f32; 2],
    /// Pointer in logical pixels.
    pub pointer: [f32; 2],
    /// UI work across the last frame, including reconciliation's layout/paint passes.
    pub work: Work,
    bindings: BTreeMap<u64, Binding>,
    modifiers: [bool; 3],
    markup: String,
    light: bool,
    present_needed: bool,
    pending_work: Work,
}

impl App {
    /// An empty window of `size` logical pixels.
    pub fn new(size: [f32; 2], docs: Vec<Doc>) -> ara_ui::Result<Self> {
        Ok(Self {
            ui: Ui::editor("<ui id='hub' />", view::CSS, size)?,
            input: InputState::default(),
            text: view::text_system()?,
            thumbnails: Thumbnails::default(),
            docs,
            size,
            pointer: [-1.; 2],
            work: Work::default(),
            bindings: BTreeMap::new(),
            modifiers: [false; 3],
            markup: String::new(),
            light: false,
            present_needed: false,
            pending_work: Work::default(),
        })
    }

    /// Reconcile changed markup, keeping native nodes, shaped text, widgets and
    /// input identity. Theme changes swap palette tokens on the same retained UI.
    pub fn rebuild(&mut self, hub: &mut Hub, now_ms: u64) -> ara_ui::Result<()> {
        let anchor = hub
            .menu
            .as_ref()
            .and_then(|id| self.ui.find(&format!("more-{id}")))
            .map(|i| {
                let r = self.ui.nodes[i].rect;
                [r.x, r.y, r.w, r.h]
            });
        let thumbnails = &self.thumbnails;
        let ctx = Ctx {
            now_ms,
            size: self.size,
            thumbnail: &|path| thumbnails.get(path),
            anchor,
            docs: &self.docs,
        };
        let page = view::page(hub, &ctx);
        self.bindings = page.bindings;
        self.thumbnails.request(&page.wanted);
        if self.markup == page.markup
            && self.ui.size == self.size
            && self.light == hub.prefs.light
            && hub.focus.is_none()
        {
            return Ok(());
        }
        if self.ui.size != self.size {
            self.ui.resize(self.size)?;
        }
        if self.light != hub.prefs.light {
            self.ui.switch_theme(if hub.prefs.light {
                EditorTheme::Light
            } else {
                EditorTheme::Dark
            })?;
            self.light = hub.prefs.light;
        }
        self.ui.reconcile(&page.markup)?;
        self.markup = page.markup;
        let work = self.ui.frame(&mut self.text)?;
        let layout_changed = work.layout_passes > 0;
        self.pending_work += work;
        if layout_changed {
            // Clamp retained scroll after layout. Zero is already valid and must
            // not dirty paint a second time for an otherwise finished frame.
            let scrolls: Vec<_> = self
                .ui
                .widgets
                .iter()
                .filter_map(|(id, widget)| {
                    if let Widget::Scroll(position) = widget
                        && *position != [0., 0.]
                    {
                        Some((*id, *position))
                    } else {
                        None
                    }
                })
                .collect();
            for (id, position) in scrolls {
                if let Some(index) = self.ui.by_id(id) {
                    self.ui.scroll_to(index, position)?;
                }
            }
        }
        if let Some(field) = hub.focus.take()
            && self.ui.find(field).is_some()
        {
            let _ = self.input.focus(&mut self.ui, stable_id(field));
            if let Some(index) = self.ui.find(field) {
                let _ = self.ui.edit(index, EditEvent::SelectAll, &mut OsClipboard);
            }
        }
        // Re-test after geometry changes; unchanged hover/focus keep their state.
        let mut clipboard = OsClipboard;
        let _ = super::input::recover(self.input.event(
            &mut self.ui,
            InputEvent::PointerMove(self.pointer),
            &mut clipboard,
        ));
        self.pending_work += self.ui.frame(&mut self.text)?;
        // Rebuild already generated a display list. Keep the presentation request
        // until the native loop consumes it, even when its next frame does no work.
        self.present_needed = true;
        Ok(())
    }

    /// Process incremental UI work and consume one presentation request. Rebuilds
    /// and input share this path, so idle frames do not submit to the GPU.
    pub fn frame(&mut self) -> ara_ui::Result<bool> {
        self.pending_work += self.ui.frame(&mut self.text)?;
        self.work = std::mem::take(&mut self.pending_work);
        let work = &self.work;
        let changed = self.present_needed
            || work.paint_nodes > 0
            || work.layout_passes > 0
            || work.restyled > 0
            || work.shaped > 0;
        self.present_needed = false;
        Ok(changed)
    }

    /// Apply activation before the next queued pointer event. A slow native frame
    /// can contain both the menu-opening click and a click on one of its items.
    pub fn dispatch_pointer(
        &mut self,
        hub: &mut Hub,
        event: InputEvent,
        now_ms: u64,
    ) -> ara_ui::Result<()> {
        self.pointer(event)?;
        let intents = self.actions();
        if !intents.is_empty() {
            for intent in intents {
                hub.intent(intent);
            }
            self.rebuild(hub, now_ms)?;
        }
        Ok(())
    }

    /// Route one pointer or wheel event.
    pub fn pointer(&mut self, event: InputEvent) -> ara_ui::Result<()> {
        if let InputEvent::PointerMove(p) | InputEvent::PointerDown(p) | InputEvent::PointerUp(p) =
            event
        {
            self.pointer = p;
        }
        self.input.event(&mut self.ui, event, &mut OsClipboard)
    }

    /// Intents raised by activated buttons and edited fields since the last call.
    pub fn actions(&mut self) -> Vec<Intent> {
        let actions: Vec<_> = self.ui.drain_actions().collect();
        actions
            .into_iter()
            .filter_map(|action| match self.bindings.get(&action.node)? {
                Binding::Intent(intent) => Some(intent.clone()),
                Binding::Field(on) => action.value.as_str().map(|v| on(v.to_owned())),
            })
            .collect()
    }

    /// Authored ID of the focused control.
    pub fn focused(&self) -> Option<&str> {
        self.input
            .focused()
            .and_then(|id| self.ui.by_id(id))
            .map(|i| self.ui.nodes[i].element.id.as_str())
    }

    /// Physical keys: shortcuts, then the focused field, then focus navigation.
    pub fn key(&mut self, hub: &Hub, scancode: i32, pressed: bool) -> ara_ui::Result<Vec<Intent>> {
        match scancode {
            225 | 229 => self.modifiers[0] = pressed,
            224 | 228 => self.modifiers[1] = pressed,
            226 | 230 => self.modifiers[2] = pressed,
            _ => {}
        }
        if !pressed {
            return Ok(Vec::new());
        }
        let [shift, ctrl, _] = self.modifiers;
        let focused = self.focused().map(str::to_owned);
        let field = focused.as_deref().is_some_and(|id| {
            self.ui
                .find(id)
                .is_some_and(|i| self.ui.nodes[i].element.tag == "field")
        });
        let mut clipboard = OsClipboard;
        match (scancode, ctrl) {
            // Escape: close the menu, drop a rename, then leave the field.
            (41, _) => {
                if hub.menu.is_some() {
                    return Ok(vec![Intent::Menu(None)]);
                }
                if hub.rename.is_some() {
                    return Ok(vec![Intent::RenameCancel]);
                }
                self.input.blur(&mut self.ui);
                return Ok(Vec::new());
            }
            (40 | 88, _) => {
                return Ok(match focused.as_deref() {
                    Some("rename-field") => vec![Intent::RenameCommit],
                    Some("create-name" | "create-destination") => vec![Intent::CreateSubmit],
                    Some(_) if !field => {
                        self.input
                            .event(&mut self.ui, InputEvent::Activate, &mut clipboard)?;
                        Vec::new()
                    }
                    _ => Vec::new(),
                });
            }
            (43, _) => {
                self.input.event(
                    &mut self.ui,
                    InputEvent::Tab { reverse: shift },
                    &mut clipboard,
                )?;
                return Ok(Vec::new());
            }
            (9, true) if hub.view == View::Projects => {
                if self.ui.find("search").is_some() {
                    let _ = self.input.focus(&mut self.ui, stable_id("search"));
                }
                return Ok(Vec::new());
            }
            (17, true) => return Ok(vec![Intent::Navigate(View::Create)]),
            _ => {}
        }
        if field {
            let edit = match (scancode, ctrl) {
                (4, true) => Some(EditEvent::SelectAll),
                (6, true) => Some(EditEvent::Copy),
                (27, true) => Some(EditEvent::Cut),
                (25, true) => Some(EditEvent::Paste),
                (29, true) => Some(if shift {
                    EditEvent::Redo
                } else {
                    EditEvent::Undo
                }),
                (28, true) => Some(EditEvent::Redo),
                (42, _) => Some(EditEvent::Backspace),
                (76, _) => Some(EditEvent::Delete),
                (79, _) => Some(EditEvent::Move {
                    forward: true,
                    select: shift,
                }),
                (80, _) => Some(EditEvent::Move {
                    forward: false,
                    select: shift,
                }),
                _ => None,
            };
            if let Some(edit) = edit {
                self.input
                    .event(&mut self.ui, InputEvent::Edit(edit), &mut clipboard)?;
            }
        }
        Ok(Vec::new())
    }
}
