//! Floating windows as native windows. The studio keeps one runtime: each floating window
//! lays out in a region beyond the main window (`dock::region`), and its own OS window shows
//! just that region (`Runtime::present_region`) and sends its input at the region's offset.
//! So a panel torn out of the dock is a real window: it moves to another monitor, and its
//! tabs drag back into the main window's dock (pointer events are mapped by the desktop
//! point while a tab is dragged).
//!
//! The layout keeps each window's frame in desktop design px (`editor.window`); moves and
//! resizes the system makes are sent back once the window has been still for a moment.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use stillkit::el::Runtime;
use stillkit::{Chrome, Event, Kind, Point, Pointer, Rect, Role, Surface, SurfaceId};

use super::{Studio, dock};

/// How long a window the system moved or resized stays still before the layout keeps it.
const SETTLE: Duration = Duration::from_millis(400);

/// The native windows of the floating ones, by floating window id.
#[derive(Default)]
pub(super) struct Portals {
    windows: BTreeMap<String, Portal>,
    /// The surface the pointer was last over (its cursor follows the runtime's).
    pointer_in: Option<SurfaceId>,
}

/// One floating window's native window.
struct Portal {
    surface: Surface,
    /// Its region in the runtime (design px).
    region: Rect,
    /// The frame the layout has for it (desktop design px).
    rect: [i32; 4],
    /// A frame the system gave it that the layout doesn't have yet, and since when.
    moved: Option<([i32; 4], Instant)>,
}

/// A window's client area on the screen (device px) and where its region starts in the
/// runtime (device px): what a desktop point maps through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    /// The client area's left, top, width and height (screen device px).
    pub client: [f32; 4],
    /// The runtime point (device px) at the client's top-left.
    pub origin: (f32, f32),
}

/// The runtime point (device px) for screen point `screen`, in the first window of
/// `windows` (topmost first) that holds it.
pub(super) fn locate(windows: &[Placement], screen: (f32, f32)) -> Option<(f32, f32)> {
    windows.iter().find_map(|w| {
        let [x, y, cw, ch] = w.client;
        let (dx, dy) = (screen.0 - x, screen.1 - y);
        (dx >= 0.0 && dy >= 0.0 && dx < cw && dy < ch).then_some((w.origin.0 + dx, w.origin.1 + dy))
    })
}

/// A surface's client area on the screen (device px): its frame from the system, less the
/// title bar above and equal borders on the sides and below.
fn client(surface: &Surface) -> Option<[f32; 4]> {
    let (x, y, w, h) = surface.placed()?;
    let (cw, ch) = surface.client_size();
    let (cw, ch) = (cw as i32, ch as i32);
    let side = ((w - cw) / 2).max(0);
    let top = (h - ch - side).max(0);
    Some([(x + side) as f32, (y + top) as f32, cw as f32, ch as f32])
}

/// The pointer an event carries, to remap.
fn pointer(event: &mut Event) -> Option<&mut Pointer> {
    match event {
        Event::Move(p)
        | Event::Wheel(p, _)
        | Event::Press(p, _)
        | Event::Release(p, _)
        | Event::DoubleClick(p, _)
        | Event::FilesHover(p)
        | Event::FilesDropped(p) => Some(p),
        _ => None,
    }
}

impl Portals {
    /// Whether surface `id` is one of the floating windows'.
    pub(super) fn owns(&self, id: SurfaceId) -> bool {
        self.windows.values().any(|p| p.surface.id() == id)
    }

    /// Opens a native window for each new floating window, closes those whose floating
    /// window went, moves any whose layout rect changed (Gather, an agent), and tells the
    /// runtime the regions (overlays opened in one fit it).
    pub(super) fn sync(&mut self, rt: &mut Runtime<Studio>, main: &Surface) -> Result<()> {
        let scale = rt.scale();
        let floats: Vec<(String, [i32; 4], Rect, String)> = {
            let s = rt.state();
            s.editor
                .state
                .floating
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    (
                        f.id.clone(),
                        f.rect,
                        dock::region(s, i, &f.id, f.rect),
                        dock::window_title(&f.root),
                    )
                })
                .collect()
        };
        let gone: Vec<String> = self
            .windows
            .keys()
            .filter(|id| !floats.iter().any(|(f, ..)| f == *id))
            .cloned()
            .collect();
        let mut changed = !gone.is_empty();
        for id in gone {
            self.windows.remove(&id);
            rt.touch(|s| s.local.desktop.sizes.remove(&id));
        }
        let background = rt.state().look.c.background.0;
        let quiet = rt.state().local.background;
        for (id, rect, region, title) in &floats {
            let frame = |r: [i32; 4]| {
                let [x, y, w, h] = r.map(|v| (v as f32 * scale).round() as i32);
                (x, y, w.max(1), h.max(1))
            };
            match self.windows.get_mut(id) {
                Some(p) => {
                    if p.rect != *rect && p.moved.is_none() {
                        p.surface.place(frame(*rect));
                        p.rect = *rect;
                    }
                    p.region = *region;
                }
                None => {
                    let surface = Surface::new(
                        Kind::App { min: (240, 160) },
                        &format!("{title} | Ara"),
                        Role::Window,
                    )?;
                    surface.set_background(background);
                    surface.place(frame(*rect));
                    if quiet {
                        surface.show();
                    } else {
                        surface.show_active(false);
                    }
                    let (cw, ch) = surface.client_size();
                    let size = (cw as f32 / scale, ch as f32 / scale);
                    let inset = surface.system_controls().map_or(0.0, |r| r.right / scale);
                    let (id2, origin) = (id.clone(), main_origin(main, scale));
                    rt.touch(|s| {
                        s.local.desktop.sizes.insert(id2, size);
                        s.local.desktop.inset = inset;
                        s.local.desktop.origin = origin;
                    });
                    self.windows.insert(
                        id.clone(),
                        Portal {
                            surface,
                            region: *region,
                            rect: *rect,
                            moved: None,
                        },
                    );
                    changed = true;
                }
            }
        }
        let regions: Vec<Rect> = self.windows.values().map(|p| p.region).collect();
        rt.set_portals(&regions);
        if changed {
            rt.update(|_| {});
        }
        Ok(())
    }

    /// Maps a pointer event from surface `id` into the runtime (device px): a floating
    /// window's at its region's offset. While a tab or asset is dragged the pointer goes by
    /// the desktop point instead, to whichever window is under it, so a tab dragged out of
    /// one window drops into another; the desktop point is kept for a tear-out.
    pub(super) fn map(
        &mut self,
        rt: &mut Runtime<Studio>,
        main: &Surface,
        id: SurfaceId,
        event: &mut Event,
    ) {
        let Some(p) = pointer(event) else {
            return;
        };
        self.pointer_in = Some(id);
        let scale = rt.scale();
        if let Some(portal) = self.windows.values().find(|w| w.surface.id() == id) {
            p.at = Point::new(
                p.at.x + portal.region.left * scale,
                p.at.y + portal.region.top * scale,
            );
        }
        let s = rt.state();
        if s.local.tab_drag.is_none() && s.local.asset_drag.is_none() {
            return;
        }
        let screen = (p.screen.0 as f32, p.screen.1 as f32);
        let mut windows: Vec<Placement> = self
            .windows
            .values()
            .filter_map(|w| {
                Some(Placement {
                    client: client(&w.surface)?,
                    origin: (w.region.left * scale, w.region.top * scale),
                })
            })
            .collect();
        windows.extend(client(main).map(|client| Placement {
            client,
            origin: (0.0, 0.0),
        }));
        if let Some((x, y)) = locate(&windows, screen) {
            p.at = Point::new(x, y);
        }
        rt.touch(|s| s.local.desktop.pointer = Some((screen.0 / scale, screen.1 / scale)));
    }

    /// Handles what a floating window's own surface reports (closing, resizing); `false`
    /// for events the runtime takes as from any window (pointer, keys).
    pub(super) fn window_event(
        &mut self,
        rt: &mut Runtime<Studio>,
        id: SurfaceId,
        event: &Event,
    ) -> bool {
        let Some((float, portal)) = self.windows.iter().find(|(_, w)| w.surface.id() == id) else {
            return false;
        };
        match event {
            Event::Close => {
                let float = float.clone();
                rt.update(|s| {
                    let panels = s
                        .editor
                        .state
                        .floating
                        .iter()
                        .find(|f| f.id == float)
                        .map(|f| dock::tabs_of(&f.root))
                        .unwrap_or_default();
                    dock::close_panels(s, &panels);
                });
            }
            Event::Resized(w, h) => {
                let scale = rt.scale();
                let size = (*w as f32 / scale, *h as f32 / scale);
                let float = float.clone();
                rt.update(|s| {
                    s.local.desktop.sizes.insert(float, size);
                });
            }
            Event::Invoke(hit) => {
                if let Some(action) = portal.surface.take_accessibility_action(*hit) {
                    rt.accessibility_action(*hit, &action);
                }
            }
            Event::Activated
            | Event::Deactivated
            | Event::DpiChanged(_)
            | Event::Leave
            | Event::CaptureLost => {}
            #[cfg(any(windows, target_os = "macos"))]
            Event::VisibilityChanged => {}
            _ => return false,
        }
        true
    }

    /// Shows each floating window's region in its window, its title bar the part the system
    /// drags, and the cursor in the window the pointer is over.
    pub(super) fn present(&mut self, rt: &mut Runtime<Studio>) -> Result<()> {
        let scale = rt.scale();
        let keys: Vec<(String, String)> = self
            .windows
            .keys()
            .map(|id| (format!("float/{id}/bar"), format!("float/{id}/close")))
            .collect();
        for (portal, (bar, close)) in self.windows.values_mut().zip(keys) {
            rt.present_region(&mut portal.surface, (0, 0), portal.region)?;
            let (ox, oy) = (portal.region.left * scale, portal.region.top * scale);
            let local = |r: Rect| Rect::new(r.left - ox, r.top - oy, r.width(), r.height());
            let mut parts = Vec::new();
            if let Some(r) = rt.find(&bar) {
                parts.push((local(r), Chrome::Caption));
            }
            if let Some(r) = rt.find(&close) {
                parts.push((local(r), Chrome::Close));
            }
            portal.surface.set_chrome(&parts);
        }
        Ok(())
    }

    /// The cursor for whichever floating window the pointer is over; `false` when that is
    /// the main window (the host sets its own).
    pub(super) fn set_cursor(&self, rt: &Runtime<Studio>) -> bool {
        let Some(id) = self.pointer_in else {
            return false;
        };
        match self.windows.values().find(|w| w.surface.id() == id) {
            Some(w) => {
                w.surface.set_cursor(rt.cursor());
                true
            }
            None => false,
        }
    }

    /// Sends the layout the frames the system moved or resized windows to, once each has
    /// been still for a moment, and keeps the main window's desktop origin current.
    pub(super) fn track(&mut self, rt: &mut Runtime<Studio>, main: &Surface) {
        let scale = rt.scale();
        let origin = main_origin(main, scale);
        if rt.state().local.desktop.origin != origin {
            rt.touch(|s| s.local.desktop.origin = origin);
        }
        let mut settled = Vec::new();
        for (id, p) in &mut self.windows {
            let Some((x, y, w, h)) = p.surface.placed() else {
                continue;
            };
            let now = [x, y, w, h].map(|v| (v as f32 / scale).round() as i32);
            match p.moved {
                _ if now == p.rect => p.moved = None,
                Some((last, since)) if last == now => {
                    if since.elapsed() >= SETTLE {
                        p.rect = now;
                        p.moved = None;
                        settled.push((id.clone(), now));
                    }
                }
                _ => p.moved = Some((now, Instant::now())),
            }
        }
        if !settled.is_empty() {
            rt.update(|s| {
                for (id, rect) in &settled {
                    dock::place_window(s, id, rect.map(|v| v as f32));
                }
            });
        }
    }
}

/// The main window's client top-left on the desktop (design px), or the origin where the
/// system doesn't say.
fn main_origin(main: &Surface, scale: f32) -> (f32, f32) {
    client(main).map_or((0.0, 0.0), |[x, y, ..]| (x / scale, y / scale))
}
