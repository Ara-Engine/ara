//! Floating OS windows for torn-off panels (D23): one SDL window, swapchain and
//! UI target each, on the shared device. The layout owns them; this module keeps
//! the OS windows in step with it, routes their input through the shell's
//! surfaces, tracks cross-window tab drags and persists moved window rects.
use super::*;

/// One floating panel window.
pub(super) struct Float {
    pub id: String,
    pub window: Window,
    pub present: Present,
    pub sdl: u32,
    /// `NativeUi` render-target key; the main window is 0.
    pub key: u32,
    pub physical: [u32; 2],
    pub rect: [i32; 4],
    /// The rect last written to the layout.
    pub saved: [i32; 4],
    pub dirty: bool,
}
/// All floating windows plus the next target key.
#[derive(Default)]
pub(super) struct Floats {
    pub windows: Vec<Float>,
    /// Create floating windows hidden (offscreen runs).
    pub hidden: bool,
    next_key: u32,
    light: Option<bool>,
}
/// Keep a rectangle reachable: if it does not overlap any display, centre it on `fallback`.
pub(super) fn recover(rect: [i32; 4], displays: &[[i32; 4]], fallback: [i32; 4]) -> [i32; 4] {
    let visible = displays.iter().any(|d| {
        let overlap_x = (rect[0] + rect[2]).min(d[0] + d[2]) - rect[0].max(d[0]);
        let overlap_y = (rect[1] + rect[3]).min(d[1] + d[3]) - rect[1].max(d[1]);
        // At least a grabbable strip of the title bar must be on screen.
        overlap_x >= 64 && overlap_y >= 24
    });
    if visible || displays.is_empty() {
        return rect;
    }
    let w = rect[2].min(fallback[2]);
    let h = rect[3].min(fallback[3]);
    [
        fallback[0] + (fallback[2] - w) / 2,
        fallback[1] + (fallback[3] - h) / 3,
        w,
        h,
    ]
}
impl Floats {
    /// Create, restyle and destroy OS windows to match the layout's floating list.
    pub fn sync(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        main: &Window,
        renderer: &Renderer,
        gpu: &mut NativeUi,
        dpi: f32,
    ) -> ara_ui::Result<bool> {
        let wanted = editor.state.floating.clone();
        let mut changed = false;
        // Close windows the layout no longer has.
        let closing: Vec<usize> = (0..self.windows.len())
            .rev()
            .filter(|i| !wanted.iter().any(|f| f.id == self.windows[*i].id))
            .collect();
        for index in closing {
            let mut float = self.windows.remove(index);
            // SAFETY: Wait for presentation before destroying its swapchain and surface.
            unsafe {
                renderer.gpu().wait_idle()?;
                float.present.destroy(renderer.gpu().context());
            }
            gpu.release_target(float.key);
            changed = true;
        }
        let displays = Window::displays();
        let main_rect = main.rect()?;
        for spec in &wanted {
            if self.windows.iter().any(|f| f.id == spec.id) {
                continue;
            }
            let rect = recover(spec.rect, &displays, main_rect);
            if rect != spec.rect {
                editor.action(
                    "editor.window",
                    json!({"id": spec.id, "rect": rect.map(|v| v as f32)}),
                )?;
            }
            let title = format!("{} · Ara", panel_titles(&spec.root));
            let window = if self.hidden {
                Window::floating_hidden(&title, rect, main)?
            } else {
                Window::floating(&title, rect, main)?
            };
            crate::native::frame::style(window.raw(), editor.state.light);
            // SAFETY: The shared device and the new window outlive this presentation.
            let present = unsafe { Present::new(renderer.gpu().context(), &window)? };
            self.next_key += 1;
            let physical = window.size()?;
            let scale = window.display_scale().max(0.25) * ui_scale(editor);
            shell.resize_surface(&spec.id, physical.map(|v| v as f32 / scale))?;
            self.windows.push(Float {
                id: spec.id.clone(),
                sdl: window.id(),
                window,
                present,
                key: self.next_key,
                physical,
                rect,
                saved: rect,
                dirty: true,
            });
            changed = true;
        }
        if changed {
            shell.rebuild(editor)?;
        }
        if self.light != Some(editor.state.light) {
            crate::native::frame::style(main.raw(), editor.state.light);
            for float in &self.windows {
                crate::native::frame::style(float.window.raw(), editor.state.light);
            }
            self.light = Some(editor.state.light);
        }
        let _ = dpi;
        Ok(changed)
    }
    pub fn by_sdl(&mut self, sdl: u32) -> Option<&mut Float> {
        self.windows.iter_mut().find(|f| f.sdl == sdl)
    }
    /// Record windows the user moved or resized (after the button is released).
    pub fn persist_rects(&mut self, editor: &mut Editor, buttons_down: bool) -> ara_ui::Result<()> {
        for float in &mut self.windows {
            float.rect = float.window.rect()?;
            if !buttons_down && float.rect != float.saved && float.rect[2] > 0 && float.rect[3] > 0
            {
                editor.action(
                    "editor.window",
                    json!({"id": float.id, "rect": float.rect.map(|v| v as f32)}),
                )?;
                float.saved = float.rect;
            }
        }
        Ok(())
    }
    /// The floating window under a desktop point, if any (topmost not tracked: first match).
    pub fn at(&self, point: [f32; 2]) -> Option<&Float> {
        self.windows.iter().find(|f| {
            let r = f.rect;
            point[0] >= r[0] as f32
                && point[1] >= r[1] as f32
                && point[0] < (r[0] + r[2]) as f32
                && point[1] < (r[1] + r[3]) as f32
        })
    }
    /// Move every floating window onto the main window's display.
    pub fn gather(&mut self, main: &Window) -> ara_ui::Result<()> {
        let m = main.rect()?;
        for (n, float) in self.windows.iter_mut().enumerate() {
            let offset = 32 * n as i32;
            float
                .window
                .set_position([m[0] + 48 + offset, m[1] + 64 + offset])?;
        }
        Ok(())
    }
    /// Destroy every window's presentation after the device is idle.
    pub fn destroy(&mut self, renderer: &Renderer) {
        for float in &mut self.windows {
            // SAFETY: Called after device idle, before the windows are dropped.
            unsafe {
                float.present.destroy(renderer.gpu().context());
            }
        }
        self.windows.clear();
    }
}
impl Floats {
    /// Logical-to-physical scale of floating window `id` (its display scale times the
    /// UI scale), if it is open.
    pub fn scale(&self, id: &str, editor: &Editor) -> Option<f32> {
        let float = self.windows.iter().find(|f| f.id == id)?;
        Some(float.window.display_scale().max(0.25) * ui_scale(editor))
    }
}
/// The UI scale preference multiplies the monitor's display scale.
pub(super) fn ui_scale(editor: &Editor) -> f32 {
    editor
        .state
        .number("appearance.uiScale", 1.)
        .clamp(0.75, 2.) as f32
}
fn panel_titles(node: &ara_ui::docking::DockNode) -> String {
    fn visit(n: &ara_ui::docking::DockNode, out: &mut Vec<String>) {
        match n {
            ara_ui::docking::DockNode::Area { tabs, .. } => out.extend(tabs.iter().map(|t| {
                let mut c = t.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            })),
            ara_ui::docking::DockNode::Split { first, second, .. } => {
                visit(first, out);
                visit(second, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(node, &mut out);
    out.join(", ")
}
#[cfg(test)]
mod tests {
    #[test]
    fn off_screen_windows_return_to_the_main_window() {
        let displays = [[0, 0, 1920, 1040]];
        let main = [100, 100, 1440, 900];
        assert_eq!(
            super::recover([200, 200, 400, 500], &displays, main),
            [200, 200, 400, 500]
        );
        let lost = super::recover([5000, 3000, 400, 500], &displays, main);
        assert!(lost[0] >= main[0] && lost[1] >= main[1]);
        // A window hanging mostly off an edge but with a grabbable strip stays put.
        assert_eq!(
            super::recover([1800, 900, 400, 500], &displays, main),
            [1800, 900, 400, 500]
        );
    }
}
