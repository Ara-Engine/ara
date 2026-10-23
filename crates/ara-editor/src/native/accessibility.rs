//! The real Windows UIA adapter, including focus, Invoke and SetValue actions.
use accesskit::{Action, ActionData, ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
use ara_ui::{
    Ui,
    input::{InputEvent, InputState},
};
use ara_ui_text::{Clipboard, EditEvent};
use std::sync::mpsc::{self, Receiver, Sender};
use windows::Win32::{
    Foundation::*,
    UI::{Shell::*, WindowsAndMessaging::*},
};
const ID: usize = 0x415241;
struct Initial(Option<TreeUpdate>);
impl ActivationHandler for Initial {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.take()
    }
}
struct Actions(Sender<ActionRequest>);
impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        let _ = self.0.send(request);
    }
}
struct State {
    adapter: accesskit_windows::Adapter,
    initial: Initial,
}
pub(super) struct Accessibility {
    state: Box<State>,
    hwnd: HWND,
    requests: Receiver<ActionRequest>,
}
unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // SAFETY: The subclass retains a stable Box<State> on the owning window thread; removal precedes Drop.
    unsafe {
        let state = &mut *(data as *mut State);
        if message == WM_GETOBJECT
            && let Some(result) = state.adapter.handle_wm_getobject(w, l, &mut state.initial) {
                return result.into();
            }
        if (message == WM_SETFOCUS || message == WM_KILLFOCUS)
            && let Some(events) = state
                .adapter
                .update_window_focus_state(message == WM_SETFOCUS)
            {
                events.raise();
            }
        DefSubclassProc(hwnd, message, w, l)
    }
}
impl Accessibility {
    pub fn new(window: *mut sdl3_sys::video::SDL_Window) -> anyhow::Result<Self> {
        // SAFETY: SDL's Win32 window property is the HWND owned by this live main-thread window.
        let hwnd = unsafe {
            HWND(sdl3_sys::properties::SDL_GetPointerProperty(
                sdl3_sys::video::SDL_GetWindowProperties(window),
                sdl3_sys::video::SDL_PROP_WINDOW_WIN32_HWND_POINTER,
                std::ptr::null_mut(),
            ))
        };
        anyhow::ensure!(!hwnd.0.is_null(), "SDL has no Win32 HWND");
        let (send, requests) = mpsc::channel();
        let mut state = Box::new(State {
            adapter: accesskit_windows::Adapter::new(hwnd, true, Actions(send)),
            initial: Initial(None),
        });
        // SAFETY: Stable userdata and matching callback signature. Callback is removed before state/window destruction.
        anyhow::ensure!(
            // SAFETY: This stable state pointer remains installed only while the Box and HWND live.
            unsafe {
                SetWindowSubclass(
                    hwnd,
                    Some(subclass),
                    ID,
                    (&mut *state as *mut State) as usize,
                )
            }
            .as_bool(),
            "AccessKit window subclass failed"
        );
        Ok(Self {
            state,
            hwnd,
            requests,
        })
    }
    pub fn update(&mut self, ui: &Ui, focus: Option<u64>) {
        let mut initial = None;
        if let Some(events) = self
            .state
            .adapter
            .update_if_active(|| ara_ui::accessibility::accessibility(ui, focus))
        {
            events.raise();
        } else {
            initial = Some(ara_ui::accessibility::accessibility(ui, focus));
        }
        self.state.initial.0 = initial;
    }
    pub fn actions(
        &mut self,
        ui: &mut Ui,
        input: &mut InputState,
        clipboard: &mut dyn Clipboard,
    ) -> ara_ui::Result<bool> {
        let mut changed = false;
        while let Ok(request) = self.requests.try_recv() {
            let id = request.target_node.0;
            if ui.by_id(id).is_none() {
                continue;
            }
            match request.action {
                Action::Focus => {
                    input.focus(ui, id)?;
                }
                Action::Click => {
                    input.focus(ui, id)?;
                    input.event(ui, InputEvent::Activate, clipboard)?;
                }
                Action::SetValue => {
                    input.focus(ui, id)?;
                    if let Some(ActionData::Value(value)) = request.data {
                        input.event(ui, InputEvent::Edit(EditEvent::SelectAll), clipboard)?;
                        input.event(
                            ui,
                            InputEvent::Edit(EditEvent::Insert(value.into())),
                            clipboard,
                        )?;
                    }
                }
                _ => continue,
            }
            changed = true;
        }
        Ok(changed)
    }
}
impl Drop for Accessibility {
    fn drop(&mut self) {
        // SAFETY: This adapter removes only its own subclass while the SDL HWND still exists.
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(subclass), ID);
        }
    }
}
