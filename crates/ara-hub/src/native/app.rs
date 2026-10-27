//! The window's retained `Ui`, reconciled from the view-model on change, and the
//! routing of pointer/keyboard input and `ara_ui` actions back to [`Intent`]s.
use super::{images::Thumbnails, input::OsClipboard, selects::Selects};
use crate::{
    model::{Dialog, Hub, Intent, View},
    view::{self, Binding, Ctx, Doc},
};
use ara_ui::{
    Ui, Work,
    input::{InputEvent, InputState},
    layers::{LayerKind, LayerStack},
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
    layers: LayerStack,
    overlay: Option<&'static str>,
    base_scope: bool,
    dialog: Option<Dialog>,
    selects: Selects,
    close_hint_shown: bool,
}

impl App {
    /// An empty window of `size` logical pixels.
    pub fn new(size: [f32; 2], docs: Vec<Doc>) -> ara_ui::Result<Self> {
        Ok(Self {
            ui: Ui::editor("<ui id='hub' />", view::stylesheet(), size)?,
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
            layers: LayerStack::default(),
            overlay: None,
            base_scope: false,
            dialog: None,
            selects: Selects::default(),
            close_hint_shown: false,
        })
    }

    /// Reconcile changed markup, keeping native nodes, shaped text, widgets and
    /// input identity. Theme changes swap palette tokens on the same retained UI.
    pub fn rebuild(&mut self, hub: &mut Hub, now_ms: u64) -> ara_ui::Result<()> {
        if self.dialog != hub.dialog {
            self.selects.dismiss(&mut self.ui, &mut self.input)?;
        }
        let anchor = hub
            .menu
            .as_ref()
            .map(|id| format!("more-{id}"))
            .and_then(|id| self.ui.find(&id))
            .map(|i| {
                let r = self.ui.nodes[i].rect;
                [r.x, r.y, r.w, r.h]
            });
        let thumbnails = &self.thumbnails;
        let close_hint = self.dialog == hub.dialog && self.close_hint();
        let ctx = Ctx {
            dialog_close_hint: close_hint,
            now_ms,
            size: self.size,
            thumbnail: &|path| thumbnails.get(path),
            anchor,
            docs: &self.docs,
        };
        let mut page = view::page(hub, &ctx);
        self.close_hint_shown = close_hint;
        self.selects
            .prepare(&mut page, &mut self.ui, &mut self.input)?;
        self.bindings = page.bindings;
        self.thumbnails.request(&page.wanted);
        if self.markup == page.markup
            && self.ui.size == self.size
            && self.light == hub.prefs.light
            && hub.focus.is_none()
            && self.dialog == hub.dialog
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
        let overlay = if hub.dialog.is_some() {
            Some("hub-dialog")
        } else {
            None
        };
        // A different dialog can reuse its root while replacing every action.
        // Keep progress refreshes stable, but give a new decision its own focus.
        let overlay_changed = self.overlay != overlay || self.dialog != hub.dialog;
        if overlay_changed {
            self.layers.close(&mut self.ui, &mut self.input)?;
        }
        self.ui.reconcile(&page.markup)?;
        self.markup = page.markup;
        let work = self.ui.frame(&mut self.text)?;
        let layout_changed = work.layout_passes > 0;
        self.pending_work += work;
        if !self.base_scope {
            self.input
                .push(&mut self.ui, stable_id("hub"), None, false)?;
            self.base_scope = true;
        }
        if overlay_changed {
            if let Some(id) = overlay {
                let kind = if id == "hub-dialog" {
                    LayerKind::Modal
                } else {
                    LayerKind::Popup
                };
                let initial = if id == "hub-dialog" {
                    dialog_focus(&self.ui)
                } else {
                    None
                };
                self.layers.open(
                    &mut self.ui,
                    &mut self.input,
                    stable_id(id),
                    kind,
                    [0., 0.],
                    initial,
                )?;
            }
            self.overlay = overlay;
            self.dialog = hub.dialog.clone();
        }
        self.selects.sync(&mut self.ui, &mut self.input)?;
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
        // Transfer phases can replace actions without changing the dialog's identity.
        if self.overlay.is_some() && self.input.focused().is_none() {
            let initial = dialog_focus(&self.ui);
            if let Some(initial) = initial {
                self.input.focus(&mut self.ui, initial)?;
            }
        }
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
        if !intents.is_empty() || self.view_dirty() {
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
        self.route(event, &mut OsClipboard)
    }

    fn route(
        &mut self,
        event: InputEvent,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> ara_ui::Result<()> {
        if let Some(event) = self
            .selects
            .event(&mut self.ui, &mut self.input, event, clipboard)?
        {
            self.layers
                .event(&mut self.ui, &mut self.input, event, clipboard)?;
        }
        Ok(())
    }

    /// Route native text through the searchable select before ordinary field editing.
    pub fn edit(
        &mut self,
        composing: &mut bool,
        edit: EditEvent,
        clipboard: &mut dyn ara_ui_text::Clipboard,
    ) -> bool {
        let closed_select = !self.selects.is_open()
            && self
                .focused()
                .and_then(|id| self.ui.find(id))
                .is_some_and(|index| self.ui.nodes[index].element.tag == "dropdown");
        super::input::deliver_edit_with(composing, edit, |edit| {
            if closed_select
                && matches!(
                    &edit,
                    EditEvent::ImeStart | EditEvent::ImePreedit { .. } | EditEvent::ImeCancel
                )
            {
                return Ok(());
            }
            self.route(InputEvent::Edit(edit), clipboard)
        })
    }

    /// Open/filter/close can change markup without accepting a domain intent.
    pub fn view_dirty(&self) -> bool {
        self.selects.dirty() || self.close_hint() != self.close_hint_shown
    }

    fn close_hint(&self) -> bool {
        self.ui.find("dialog-close").is_some_and(|index| {
            let button = &self.ui.nodes[index];
            button.rect.contains(self.pointer[0], self.pointer[1])
                && button.clip.contains(self.pointer[0], self.pointer[1])
        })
    }

    /// Inspect the same typed choice state used by native input and captures.
    pub fn select(&self, control: &str) -> Option<&ara_ui::select::Select<Intent>> {
        self.selects.get(control)
    }

    /// Intents raised by activated buttons and edited fields since the last call.
    pub fn actions(&mut self) -> Vec<Intent> {
        let mut intents =
            super::input::recover(self.selects.actions(&mut self.ui, &mut self.input))
                .unwrap_or_default();
        let actions: Vec<_> = self.ui.drain_actions().collect();
        intents.extend(actions.into_iter().filter_map(|action| {
            match self.bindings.get(&action.node)? {
                Binding::Intent(intent) => Some(intent.clone()),
                Binding::Field(on) => action.value.as_str().map(|v| on(v.to_owned())),
            }
        }));
        intents
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
        if !ctrl {
            let key = match scancode {
                74 => Some(ara_ui::select::Key::Home),
                77 => Some(ara_ui::select::Key::End),
                81 => Some(ara_ui::select::Key::Down),
                82 => Some(ara_ui::select::Key::Up),
                40 | 88 => Some(ara_ui::select::Key::Enter),
                41 => Some(ara_ui::select::Key::Escape),
                _ => None,
            };
            if let Some(key) = key
                && self.selects.key(&mut self.ui, &mut self.input, key)?
            {
                return Ok(self.actions());
            }
        }
        let field = focused.as_deref().is_some_and(|id| {
            self.ui
                .find(id)
                .is_some_and(|i| self.ui.nodes[i].element.tag == "field")
        });
        let mut clipboard = OsClipboard;
        match (scancode, ctrl) {
            // Escape: close the menu, drop a rename, then leave the field.
            (41, _) => {
                if hub.dialog.is_some() {
                    return Ok(vec![Intent::DismissDialog]);
                }
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
                        self.route(InputEvent::Activate, &mut clipboard)?;
                        Vec::new()
                    }
                    _ => Vec::new(),
                });
            }
            (43, _) => {
                self.route(InputEvent::Tab { reverse: shift }, &mut clipboard)?;
                return Ok(Vec::new());
            }
            (9, true)
                if hub.view == View::Projects
                    && hub.dialog.is_none()
                    && !self.selects.is_open() =>
            {
                if self.ui.find("search").is_some() {
                    let _ = self.input.focus(&mut self.ui, stable_id("search"));
                }
                return Ok(Vec::new());
            }
            (17, true) if hub.dialog.is_none() && !self.selects.is_open() => {
                return Ok(vec![Intent::Navigate(View::Create)]);
            }
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
                self.route(InputEvent::Edit(edit), &mut clipboard)?;
            }
        }
        Ok(Vec::new())
    }
}

/// Focus a safe action when a dialog first opens or replaces a transfer control.
fn dialog_focus(ui: &Ui) -> Option<u64> {
    [
        "dialog-cancel",
        "dialog-later",
        "dialog-error-dismiss",
        "dialog-done",
        "dialog-close",
    ]
    .into_iter()
    .find(|id| {
        ui.find(id)
            .is_some_and(|index| !ui.nodes[index].state.disabled)
    })
    .map(stable_id)
}
