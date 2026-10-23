//! The window's retained `Ui`, rebuilt from the view-model on change, and the
//! routing of pointer/keyboard input and `ara_ui` actions back to [`Intent`]s.
use super::{images::Thumbnails, input::OsClipboard};
use crate::{
    model::{Hub, Intent, View},
    view::{self, Binding, Ctx, Doc},
};
use ara_ui::{
    Ui,
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
    bindings: BTreeMap<u64, Binding>,
    modifiers: [bool; 3],
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
            bindings: BTreeMap::new(),
            modifiers: [false; 3],
        })
    }

    /// Regenerate the page from `hub`, keeping field carets, scroll offsets, focus
    /// and hover across the rebuild.
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
        let mut ui = Ui::editor(&page.markup, view::CSS, self.size)?;
        // Always switch: it also marks every node for restyle, which is what resolves
        // `color: inherit` (icons and labels in colored buttons) on the first frame.
        ui.switch_theme(if hub.prefs.light {
            EditorTheme::Light
        } else {
            EditorTheme::Dark
        })?;
        let old = std::mem::replace(&mut self.ui, ui);
        let mut scrolls = Vec::new();
        for (id, widget) in old.widgets {
            match (widget, self.ui.widgets.get_mut(&id)) {
                (Widget::Input(kept), Some(Widget::Input(fresh)))
                    if kept.value() == fresh.value() =>
                {
                    *fresh = kept;
                }
                (Widget::Scroll(position), Some(Widget::Scroll(_))) => scrolls.push((id, position)),
                _ => {}
            }
        }
        self.ui.frame(&mut self.text)?;
        for (id, position) in scrolls {
            if let Some(index) = self.ui.by_id(id) {
                self.ui.scroll_to(index, position)?;
            }
        }
        if let Some(field) = hub.focus.take()
            && self.ui.find(field).is_some()
        {
            let _ = self.input.focus(&mut self.ui, stable_id(field));
            if let Some(index) = self.ui.find(field) {
                let _ = self.ui.edit(index, EditEvent::SelectAll, &mut OsClipboard);
            }
        } else if let Some(id) = self.input.focused() {
            let _ = self.input.focus(&mut self.ui, id);
        }
        // Hover lives in InputState by identity; re-derive it on the new nodes.
        let mut clipboard = OsClipboard;
        self.input.event(
            &mut self.ui,
            InputEvent::PointerMove([-1e5; 2]),
            &mut clipboard,
        )?;
        self.input.event(
            &mut self.ui,
            InputEvent::PointerMove(self.pointer),
            &mut clipboard,
        )?;
        self.ui.frame(&mut self.text)?;
        self.ui.drain_actions().for_each(drop);
        self.bindings = page.bindings;
        self.thumbnails.request(&page.wanted);
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
