//! The Win32 window: a borderless, DWM-framed 480 × 200 card centred on the
//! monitor under the pointer, per-monitor DPI aware, dragged by any empty area.
//! The install runs on a worker thread that sends [`View`]s; its questions block
//! it until a button answers. Closing is ignored while setup is busy.
use super::{
    Action, View,
    canvas::RectF,
    paint::{self, Frame, Pointer, Surface},
};
use crate::{
    args::Args,
    flow::{self, Host, Outcome, Plan, Resolution, Step},
    processes::Blocker,
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GlobalFree, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_ROUND, DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
        },
        Gdi::{
            BeginPaint, BitBlt, EndPaint, GetMonitorInfoW, InvalidateRect,
            MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, SRCCOPY,
            ScreenToClient,
        },
    },
    System::{
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
    },
    UI::{
        Controls::{MARGINS, WM_MOUSELEAVE},
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
            SetProcessDpiAwarenessContext,
        },
        Input::KeyboardAndMouse::{
            ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE,
            VK_RETURN,
        },
        WindowsAndMessaging::{
            CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
            GWLP_USERDATA, GetClientRect, GetCursorPos, GetMessageW, GetWindowLongPtrW, HTCAPTION,
            HTCLIENT, IDC_ARROW, KillTimer, LoadCursorW, LoadIconW, MSG, PostMessageW,
            PostQuitMessage, RegisterClassExW, SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE,
            SWP_NOZORDER, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
            WM_APP, WM_CLOSE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
            WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCCREATE, WM_NCHITTEST, WM_PAINT,
            WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_EX_APPWINDOW, WS_MINIMIZEBOX, WS_POPUP,
            WS_SYSMENU,
        },
    },
};

/// Posted by the worker after queueing an [`Event`].
const WAKE: u32 = WM_APP + 1;
/// The progress sheen's timer.
const ANIMATE: usize = 1;
const CF_UNICODETEXT: u32 = 13;

enum Event {
    Show(View),
    Close,
}

/// The worker's [`Host`]: sends views to the window and waits for its answers.
struct Remote {
    events: Sender<Event>,
    answers: Receiver<Action>,
    window: usize,
    plan: Option<Plan>,
    sent: Instant,
}

impl Remote {
    fn send(&self, event: Event) {
        if self.events.send(event).is_ok() {
            // SAFETY: posting to a window handle is safe from any thread; a
            // destroyed window just fails the post.
            unsafe { PostMessageW(self.window as HWND, WAKE, 0, 0) };
        }
    }
}

impl Host for Remote {
    fn plan(&mut self, plan: &Plan) {
        self.plan = Some(plan.clone());
    }
    fn progress(&mut self, step: Step<'_>) {
        // Byte progress arrives per MiB; about 30 updates a second is plenty.
        let streaming = matches!(step, Step::Verify { .. } | Step::Extract { .. });
        if streaming && self.sent.elapsed() < Duration::from_millis(33) {
            return;
        }
        self.sent = Instant::now();
        self.send(Event::Show(View::progress(self.plan.as_ref(), step)));
    }
    fn confirm_replace(&mut self, plan: &Plan, installed: &str) -> bool {
        self.send(Event::Show(View::confirm(plan, installed)));
        loop {
            match self.answers.recv() {
                Ok(Action::Replace) => return true,
                Ok(Action::Cancel) | Err(_) => return false,
                Ok(_) => {}
            }
        }
    }
    fn resolve(&mut self, _: &Plan, blockers: &[Blocker]) -> Resolution {
        self.send(Event::Show(View::blocked(blockers)));
        loop {
            match self.answers.recv() {
                Ok(Action::Retry) => return Resolution::Retry,
                Ok(Action::StopService) => return Resolution::StopService,
                Ok(Action::Cancel) | Err(_) => return Resolution::Cancel,
                Ok(_) => {}
            }
        }
    }
}

/// Window state, owned by the window procedure through `GWLP_USERDATA`.
struct App {
    view: View,
    /// The last progress view, shown again the moment a question is answered.
    busy: View,
    pointer: Pointer,
    hits: Vec<(Action, RectF)>,
    scale: f32,
    frame: Frame,
    tracking: bool,
    events: Receiver<Event>,
    answers: Sender<Action>,
}

/// Shows the window and installs on a worker thread, or shows `fixed` (an error
/// found before installing) without one. Returns true once setup installed.
pub fn run(exe: PathBuf, args: Args, fixed: Option<View>) -> bool {
    // SAFETY: called before any window exists; failure leaves system DPI scaling.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let (events, receive) = mpsc::channel();
    let (answer, answers) = mpsc::channel();
    let app = Box::into_raw(Box::new(App {
        busy: View::preparing(),
        view: fixed.clone().unwrap_or_else(View::preparing),
        pointer: Pointer::default(),
        hits: Vec::new(),
        scale: 1.0,
        frame: Frame::Square,
        tracking: false,
        events: receive,
        answers: answer,
    }));
    // SAFETY: `app` stays valid until it is reclaimed below, after the message
    // loop ends and the window (the only other user) is destroyed.
    let Some(window) = (unsafe { create(app) }) else {
        // SAFETY: the window was never created, so this is the only owner.
        drop(unsafe { Box::from_raw(app) });
        return false;
    };
    let worker = fixed.is_none().then(|| {
        let window = window as usize;
        std::thread::spawn(move || {
            let mut host = Remote {
                events,
                answers,
                window,
                plan: None,
                sent: Instant::now(),
            };
            let (event, installed) = match flow::run(&exe, &args, &mut host) {
                Ok(Outcome::Installed(installed)) => (
                    Event::Show(View::done(&installed, args.no_system_integration)),
                    true,
                ),
                Ok(Outcome::Cancelled) => (Event::Close, false),
                Err(error) => (Event::Show(View::failed(host.plan.as_ref(), &error)), false),
            };
            host.send(event);
            installed
        })
    });
    // SAFETY: a standard message loop on the thread that created the window.
    unsafe {
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    // SAFETY: the window is destroyed, so nothing else refers to `app`. Dropping
    // it closes the answer channel, which unblocks a worker waiting for one.
    drop(unsafe { Box::from_raw(app) });
    worker.is_some_and(|worker| worker.join().unwrap_or(false))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Registers the class and creates the window centred on the pointer's monitor.
///
/// # Safety
/// `app` must stay valid until the window is destroyed.
unsafe fn create(app: *mut App) -> Option<HWND> {
    // SAFETY: plain Win32 calls with NUL-terminated strings that outlive them;
    // `app` is handed to WM_NCCREATE as the creation parameter.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("AraSetupWindow");
        let title = wide("Ara Setup");
        // Resource 1 is the Ara icon embedded by build.rs.
        let icon = LoadIconW(instance, std::ptr::without_provenance(1));
        let registered = RegisterClassExW(&WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(procedure),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: icon,
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: icon,
        });
        if registered == 0 {
            return None;
        }
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        let mut monitor = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..std::mem::zeroed()
        };
        GetMonitorInfoW(
            MonitorFromPoint(cursor, MONITOR_DEFAULTTOPRIMARY),
            &mut monitor,
        );
        let work = monitor.rcWork;
        let window = CreateWindowExW(
            WS_EX_APPWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            work.left,
            work.top,
            paint::WIDTH as i32,
            paint::HEIGHT as i32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            app.cast(),
        );
        if window.is_null() {
            return None;
        }
        // The window now sits on the pointer's monitor: size it for that DPI.
        (*app).scale = GetDpiForWindow(window).max(96) as f32 / 96.0;
        let (width, height) = paint::size((*app).scale);
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 2,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        (*app).frame = style(window);
        sync_timer(window, &(*app).view);
        ShowWindow(window, SW_SHOW);
        Some(window)
    }
}

/// Dark DWM frame: rounded corners and a theme border on Windows 11 (where the
/// return is `Frame::System`), the shadow everywhere, a painted border elsewhere.
///
/// # Safety
/// `window` must be a live window of this thread.
unsafe fn style(window: HWND) -> Frame {
    let set = |attribute: i32, value: u32| {
        // SAFETY: the attribute reads a u32 from a live local for the call only.
        unsafe {
            DwmSetWindowAttribute(
                window,
                attribute as u32,
                (&raw const value).cast(),
                size_of::<u32>() as u32,
            )
        }
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    let rounded = set(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND as u32) >= 0;
    set(DWMWA_BORDER_COLOR, paint::border().colorref());
    // A 1 px extended frame keeps the DWM shadow without a visible caption;
    // painting is fully opaque, so it never shows through.
    let margins = MARGINS {
        cxLeftWidth: 1,
        cxRightWidth: 1,
        cyTopHeight: 1,
        cyBottomHeight: 1,
    };
    // SAFETY: `margins` outlives the call.
    unsafe { DwmExtendFrameIntoClientArea(window, &margins) };
    if rounded {
        Frame::System
    } else {
        Frame::Square
    }
}

/// The sheen animates only while setup is busy.
///
/// # Safety
/// `window` must be a live window of this thread.
unsafe fn sync_timer(window: HWND, view: &View) {
    // SAFETY: timers belong to the live window.
    unsafe {
        if matches!(view, View::Busy(_)) {
            SetTimer(window, ANIMATE, 33, None);
        } else {
            KillTimer(window, ANIMATE);
        }
    }
}

fn point(lparam: LPARAM) -> (f32, f32) {
    (
        f32::from((lparam & 0xffff) as u16 as i16),
        f32::from(((lparam >> 16) & 0xffff) as u16 as i16),
    )
}

impl App {
    fn hit(&self, (x, y): (f32, f32)) -> Option<Action> {
        self.hits
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(action, _)| *action)
    }

    /// Shows `view`, keeping the latest progress for answered questions.
    fn show(&mut self, window: HWND, view: View) {
        if matches!(view, View::Busy(_)) {
            self.busy = view.clone();
        }
        self.view = view;
        self.pointer.pressed = None;
        // SAFETY: `window` is this app's live window.
        unsafe {
            sync_timer(window, &self.view);
            InvalidateRect(window, std::ptr::null(), 0);
        }
    }

    fn activate(&mut self, window: HWND, action: Action) {
        match action {
            Action::Replace | Action::Retry | Action::StopService => {
                if self.answers.send(action).is_ok() {
                    self.show(window, self.busy.clone());
                }
            }
            Action::Cancel => {
                // The worker answers with Close; without one there is nothing to cancel.
                if self.answers.send(action).is_err() {
                    // SAFETY: destroying our own live window.
                    unsafe { DestroyWindow(window) };
                }
            }
            Action::CopyDetails => {
                if let View::Error {
                    details, copied, ..
                } = &mut self.view
                {
                    // SAFETY: `window` is live and owns the clipboard while open.
                    *copied = unsafe { copy_text(window, details) };
                    // SAFETY: as above.
                    unsafe { InvalidateRect(window, std::ptr::null(), 0) };
                }
            }
            Action::Close => {
                // SAFETY: destroying our own live window.
                unsafe { DestroyWindow(window) };
            }
            Action::Launch => {
                let View::Done {
                    launch: Some(hub), ..
                } = &self.view
                else {
                    return;
                };
                let mut command = std::process::Command::new(hub);
                if let Some(directory) = hub.parent() {
                    command.current_dir(directory);
                }
                match command.spawn() {
                    // SAFETY: destroying our own live window.
                    Ok(_) => unsafe {
                        DestroyWindow(window);
                    },
                    Err(error) => {
                        let details = format!("Starting {}: {error}", hub.display());
                        self.show(
                            window,
                            View::Error {
                                title: "Couldn't start Ara Hub".into(),
                                code: None,
                                message: error.to_string(),
                                details,
                                copied: false,
                            },
                        );
                    }
                }
            }
        }
    }
}

/// Puts `text` on the clipboard as Unicode text.
///
/// # Safety
/// `window` must be a live window of this thread.
unsafe fn copy_text(window: HWND, text: &str) -> bool {
    let wide = wide(&text.replace('\n', "\r\n"));
    // SAFETY: the clipboard is opened for `window` and always closed; the global
    // block is sized for `wide` and owned by the clipboard once set (else freed).
    unsafe {
        if OpenClipboard(window) == 0 {
            return false;
        }
        EmptyClipboard();
        let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        let mut placed = false;
        if !memory.is_null() {
            let target = GlobalLock(memory).cast::<u16>();
            if !target.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), target, wide.len());
                GlobalUnlock(memory);
                placed = !SetClipboardData(CF_UNICODETEXT, memory as HANDLE).is_null();
            }
            if !placed {
                GlobalFree(memory);
            }
        }
        CloseClipboard();
        placed
    }
}

unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Win32 delivers messages for `window` on its thread. GWLP_USERDATA
    // holds the `App` installed at WM_NCCREATE, valid until the message loop ends.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
            return DefWindowProcW(window, message, wparam, lparam);
        }
        let app = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut App;
        let Some(app) = app.as_mut() else {
            return DefWindowProcW(window, message, wparam, lparam);
        };
        match message {
            // The whole window is client area: no caption or border is drawn.
            WM_NCCALCSIZE if wparam != 0 => 0,
            WM_NCHITTEST => {
                let (x, y) = point(lparam);
                let mut at = POINT {
                    x: x as i32,
                    y: y as i32,
                };
                ScreenToClient(window, &mut at);
                if app.hit((at.x as f32, at.y as f32)).is_some() {
                    HTCLIENT as LRESULT
                } else {
                    HTCAPTION as LRESULT
                }
            }
            WM_MOUSEMOVE => {
                let hover = app.hit(point(lparam));
                if !app.tracking {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: window,
                        dwHoverTime: 0,
                    };
                    app.tracking = TrackMouseEvent(&mut track) != 0;
                }
                if hover != app.pointer.hover {
                    app.pointer.hover = hover;
                    InvalidateRect(window, std::ptr::null(), 0);
                }
                0
            }
            WM_MOUSELEAVE => {
                app.tracking = false;
                app.pointer.hover = None;
                InvalidateRect(window, std::ptr::null(), 0);
                0
            }
            WM_LBUTTONDOWN => {
                app.pointer.pressed = app.hit(point(lparam));
                app.pointer.hover = app.pointer.pressed;
                SetCapture(window);
                InvalidateRect(window, std::ptr::null(), 0);
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                let hit = app.hit(point(lparam));
                let pressed = app.pointer.pressed.take();
                InvalidateRect(window, std::ptr::null(), 0);
                if let Some(action) = hit.filter(|action| Some(*action) == pressed) {
                    app.activate(window, action);
                }
                0
            }
            WM_KEYDOWN => {
                let action = match wparam as u16 {
                    VK_RETURN => app.view.primary(),
                    VK_ESCAPE => app.view.dismiss(),
                    _ => None,
                };
                if let Some(action) = action {
                    app.activate(window, action);
                }
                0
            }
            WM_CLOSE => {
                if let Some(action) = app.view.dismiss() {
                    app.activate(window, action);
                }
                0
            }
            WM_TIMER => {
                app.pointer.tick = app.pointer.tick.wrapping_add(1);
                InvalidateRect(window, std::ptr::null(), 0);
                0
            }
            WAKE => {
                while let Ok(event) = app.events.try_recv() {
                    match event {
                        Event::Show(view) => app.show(window, view),
                        Event::Close => {
                            DestroyWindow(window);
                            break;
                        }
                    }
                }
                0
            }
            WM_DPICHANGED => {
                app.scale = f32::from((wparam >> 16) as u16).max(96.0) / 96.0;
                let suggested = &*(lparam as *const RECT);
                let (width, height) = paint::size(app.scale);
                SetWindowPos(
                    window,
                    std::ptr::null_mut(),
                    suggested.left,
                    suggested.top,
                    width,
                    height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                InvalidateRect(window, std::ptr::null(), 0);
                0
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut paint: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(window, &mut paint);
                let mut client: RECT = std::mem::zeroed();
                GetClientRect(window, &mut client);
                let (width, height) = (client.right - client.left, client.bottom - client.top);
                if let Some(mut surface) = Surface::new(width, height) {
                    app.hits =
                        paint::paint(&mut surface, &app.view, app.pointer, app.scale, app.frame);
                    BitBlt(dc, 0, 0, width, height, surface.dc(), 0, 0, SRCCOPY);
                }
                EndPaint(window, &paint);
                0
            }
            WM_DESTROY => {
                KillTimer(window, ANIMATE);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}
