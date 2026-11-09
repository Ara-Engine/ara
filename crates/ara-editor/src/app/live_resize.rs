//! Live window resizing from the custom-chrome border (Windows).
//!
//! Windows runs a modal loop while the OS resizes a window. That loop blocks the
//! editor's event loop: nothing re-renders until the mouse is released, and DWM
//! stretches the last frame meanwhile. The editor therefore resizes its own window
//! from the border zones (`WindowChrome::client_resize`) and keeps rendering at every
//! size. Each frame `step` reads the cursor and yields the size to lay out and
//! present at; `apply` moves the window right after that present, so the new frame
//! and the new window bounds reach the compositor together (moving first left a
//! frame of mismatched content at the border). `reach` bounds the gesture so the
//! swapchain is sized once per drag (`NativeUi::resize_present`), not every frame.
use ara_platform::{Event, SystemCursor, Window};

/// Which borders a resize gesture moves.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Edge {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

/// Border hover and the active resize gesture for the main window.
#[derive(Default)]
pub(super) struct EdgeResize {
    hover: Option<Edge>,
    #[cfg(windows)]
    drag: Option<Drag>,
}

#[cfg(windows)]
struct Drag {
    edge: Edge,
    cursor: windows::Win32::Foundation::POINT,
    rect: windows::Win32::Foundation::RECT,
    min: [i32; 2],
    /// Outer window size minus the drawable size (an invisible frame, if any).
    frame: [i32; 2],
    /// The largest drawable size the gesture can reach on the window's monitor.
    reach: [u32; 2],
    /// The outer rect `step` computed and `apply` has not set yet.
    pending: Option<windows::Win32::Foundation::RECT>,
    /// The outer rect last set.
    applied: windows::Win32::Foundation::RECT,
}

impl EdgeResize {
    /// Whether the editor drives resizing on this platform.
    pub(super) const ENABLED: bool = cfg!(windows);

    /// Feed a main-window event; true when the gesture consumed it.
    pub(super) fn event(&mut self, window: &Window, event: &Event) -> bool {
        match *event {
            Event::MouseMotion { position, .. } => {
                if !self.active() {
                    self.hover = edge_at(window, position);
                }
                self.active()
            }
            Event::MouseButton {
                button: 1,
                pressed: true,
            } => match self.hover {
                Some(edge) => self.begin(window, edge),
                None => false,
            },
            Event::MouseButton {
                button: 1,
                pressed: false,
            } if self.active() => {
                self.end(window);
                true
            }
            Event::FocusLost => {
                self.end(window);
                false
            }
            _ => false,
        }
    }

    /// Whether a resize gesture is in progress.
    pub(super) fn active(&self) -> bool {
        #[cfg(windows)]
        {
            self.drag.is_some()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// The resize cursor over (or while dragging) a border.
    pub(super) fn cursor(&self) -> Option<SystemCursor> {
        #[cfg(windows)]
        let edge = self.drag.as_ref().map(|d| d.edge).or(self.hover);
        #[cfg(not(windows))]
        let edge = self.hover;
        edge.map(|e| match (e.left || e.right, e.top || e.bottom) {
            (true, true) if (e.left && e.top) || (e.right && e.bottom) => SystemCursor::ResizeNwse,
            (true, true) => SystemCursor::ResizeNesw,
            (true, false) => SystemCursor::ResizeEw,
            _ => SystemCursor::ResizeNs,
        })
    }

    /// The largest drawable size (pixels) the active gesture can reach: the
    /// presentation keeps one swapchain this large for the whole drag.
    pub(super) fn reach(&self) -> Option<[u32; 2]> {
        #[cfg(windows)]
        {
            self.drag.as_ref().map(|d| d.reach)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    #[cfg(windows)]
    fn begin(&mut self, window: &Window, edge: Edge) -> bool {
        use windows::Win32::{
            Foundation::{POINT, RECT},
            Graphics::Gdi::{
                GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
            },
            UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect},
        };
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        let Ok(drawable) = window.size() else {
            return false;
        };
        let (mut cursor, mut rect) = (POINT::default(), RECT::default());
        // SAFETY: SDL owns this live HWND on the current thread; both calls only fill
        // the provided out-parameters.
        if unsafe { GetCursorPos(&mut cursor).is_err() || GetWindowRect(hwnd, &mut rect).is_err() }
        {
            return false;
        }
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `hwnd` is live; the call fills `monitor`, whose size is set above.
        let found = unsafe {
            GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
        };
        let work = if found.as_bool() {
            monitor.rcWork
        } else {
            rect
        };
        let frame = [
            (rect.right - rect.left) - drawable[0] as i32,
            (rect.bottom - rect.top) - drawable[1] as i32,
        ];
        // A moved edge can reach the monitor's work area; the other edge stays.
        let reach = [
            match (edge.left, edge.right) {
                (true, _) => rect.right - work.left,
                (_, true) => work.right - rect.left,
                _ => rect.right - rect.left,
            } - frame[0],
            match (edge.top, edge.bottom) {
                (true, _) => rect.bottom - work.top,
                (_, true) => work.bottom - rect.top,
                _ => rect.bottom - rect.top,
            } - frame[1],
        ];
        let scale = window.display_scale().max(1.);
        self.drag = Some(Drag {
            edge,
            cursor,
            rect,
            min: [(640. * scale) as i32, (400. * scale) as i32],
            frame,
            reach: [0, 1].map(|i| (reach[i].max(0) as u32).max(drawable[i])),
            pending: None,
            applied: rect,
        });
        // SAFETY: Main-thread SDL call; capture keeps motion and the release flowing
        // while the pointer is outside the window.
        unsafe { sdl3_sys::mouse::SDL_CaptureMouse(true) };
        true
    }
    #[cfg(not(windows))]
    fn begin(&mut self, _: &Window, _: Edge) -> bool {
        false
    }

    fn end(&mut self, window: &Window) {
        self.apply(window);
        #[cfg(windows)]
        if self.drag.take().is_some() {
            // SAFETY: Main-thread SDL call releasing the capture taken in `begin`.
            unsafe { sdl3_sys::mouse::SDL_CaptureMouse(false) };
        }
    }

    /// Read the cursor once per frame, before the loop sizes its layout: the
    /// drawable size to lay out and present at when the window is about to change.
    /// The reach grows if the cursor leaves the monitor it was measured on.
    pub(super) fn step(&mut self) -> Option<[u32; 2]> {
        #[cfg(windows)]
        {
            use windows::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};
            let drag = self.drag.as_mut()?;
            let mut cursor = POINT::default();
            // SAFETY: Fills the out-parameter only.
            if unsafe { GetCursorPos(&mut cursor) }.is_err() {
                return None;
            }
            let (dx, dy) = (cursor.x - drag.cursor.x, cursor.y - drag.cursor.y);
            let mut r = drag.rect;
            if drag.edge.left {
                r.left = (r.left + dx).min(r.right - drag.min[0]);
            }
            if drag.edge.right {
                r.right = (r.right + dx).max(r.left + drag.min[0]);
            }
            if drag.edge.top {
                r.top = (r.top + dy).min(r.bottom - drag.min[1]);
            }
            if drag.edge.bottom {
                r.bottom = (r.bottom + dy).max(r.top + drag.min[1]);
            }
            if r == drag.applied {
                drag.pending = None;
                return None;
            }
            drag.pending = Some(r);
            let size = [
                (r.right - r.left - drag.frame[0]).max(1) as u32,
                (r.bottom - r.top - drag.frame[1]).max(1) as u32,
            ];
            drag.reach = [drag.reach[0].max(size[0]), drag.reach[1].max(size[1])];
            Some(size)
        }
        #[cfg(not(windows))]
        None
    }

    /// Move the window to the rect `step` computed; call right after presenting the
    /// frame laid out at its size (or once nothing will present this frame).
    pub(super) fn apply(&mut self, window: &Window) {
        #[cfg(windows)]
        {
            use windows::Win32::UI::WindowsAndMessaging::{
                SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos,
            };
            let Some(drag) = self.drag.as_mut() else {
                return;
            };
            let (Some(r), Some(hwnd)) = (drag.pending.take(), hwnd(window)) else {
                return;
            };
            drag.applied = r;
            // SAFETY: Live HWND owned by SDL on this thread. One atomic move and size,
            // so dragging the left or top edge never shifts the opposite edge; the
            // frame just presented already shows this size, so no bits are copied.
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
                )
            };
        }
        #[cfg(not(windows))]
        let _ = window;
    }
}

/// The border zone under a window-coordinate point, if the window is restored.
fn edge_at(window: &Window, point: [f32; 2]) -> Option<Edge> {
    let chrome = window.chrome().filter(|c| c.client_resize)?;
    if window.maximized() {
        return None;
    }
    let [_, _, w, h] = window.rect().ok()?;
    let b = chrome.border;
    let edge = Edge {
        left: point[0] < b,
        right: point[0] >= w as f32 - b,
        top: point[1] < b,
        bottom: point[1] >= h as f32 - b,
    };
    (edge.left || edge.right || edge.top || edge.bottom).then_some(edge)
}

#[cfg(windows)]
fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    // SAFETY: SDL owns the HWND property of this live window.
    let raw = unsafe {
        sdl3_sys::properties::SDL_GetPointerProperty(
            sdl3_sys::video::SDL_GetWindowProperties(window.raw()),
            sdl3_sys::video::SDL_PROP_WINDOW_WIN32_HWND_POINTER,
            std::ptr::null_mut(),
        )
    };
    (!raw.is_null()).then_some(windows::Win32::Foundation::HWND(raw))
}

#[cfg(test)]
mod timing {
    use crate::{editor::Editor, shell::Shell};
    use serde_json::json;
    use std::time::Instant;

    /// What each live-resize step costs the UI thread: `Ui::resize` plus a full Shell
    /// rebuild (the loop's old path) against `Ui::resize` alone, then layout + paint.
    /// `cargo test -p ara-editor --lib live_resize::timing -- --ignored --nocapture`
    #[test]
    #[ignore = "prints timings"]
    fn ui_cost_per_resize_step() -> ara_ui::Result<()> {
        let root = crate::test_target().join(format!("live-resize-timing-{}", std::process::id()));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))?;
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"name":"Resize timing","main":"levels/main.scene","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
            ),
        )?;
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_resizetim","records":{}}),
            ),
        )?;
        let mut editor = Editor::open_with(&root, Some(root.join(".ara/user.json")))?;
        let mut shell = Shell::new(&mut editor, [1440., 900.])?;
        shell.frame()?;
        for rebuild in [true, false] {
            let mut ms = Vec::new();
            for i in 0..120u32 {
                let size = [1440. + (i % 40) as f32 * 3., 900. + (i % 30) as f32 * 2.];
                let at = Instant::now();
                shell.ui.resize(size)?;
                if rebuild {
                    shell.rebuild(&mut editor)?;
                }
                shell.frame()?;
                ms.push(at.elapsed().as_secs_f64() * 1000.);
            }
            ms.sort_by(f64::total_cmp);
            println!(
                "resize{}: p50 {:.2} ms, p95 {:.2}, max {:.2}",
                if rebuild { " + rebuild" } else { " only" },
                ms[ms.len() / 2],
                ms[ms.len() * 95 / 100],
                ms[ms.len() - 1]
            );
        }
        Ok(())
    }
}
