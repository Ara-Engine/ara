//! The one orientation aid (Appendix C): a 56 px transparent widget, top-right
//! with a 12 px inset. Positive axes are 14 px filled, lettered caps with stems;
//! negative axes are 10 px hollow rings. Caps are real accessible buttons.
use super::*;
use ara_ui::{paint::Draw, source::stable_id, style::Color};
use glam::Vec3;

const SIZE: f32 = 56.;
const RADIUS: f32 = 20.;
impl Shell {
    pub(super) fn axis_markup(&mut self, editor: &Editor, out: &mut String) {
        out.push_str(&format!("<panel id='view-cube' style='position:absolute;right:12px;top:12px;width:{SIZE}px;height:{SIZE}px'>"));
        let camera = editor.state.camera;
        self.axis_camera = camera;
        let mut caps = caps(&camera);
        caps.sort_by(|a, b| a.2.total_cmp(&b.2));
        let c = SIZE * 0.5;
        self.bind_command("view-iso", "view.camera.default", commands::Target::None);
        out.push_str(&format!("<button id='view-iso' class='view-sphere' style='position:absolute;left:{}px;top:{}px;width:10px;height:10px' label='Perspective view' description='Return to the default perspective' action='Dispatch' />",c-5.,c-5.));
        for (axis, sign, _, p) in &caps {
            let id = cap_id(*axis, *sign);
            let command = match (*axis, *sign) {
                (0, true) => "view.camera.right",
                (0, false) => "view.camera.left",
                (1, true) => "view.camera.top",
                (1, false) => "view.camera.bottom",
                (2, true) => "view.camera.front",
                _ => "view.camera.back",
            };
            self.bind_command(&id, command, commands::Target::None);
            let label = ["X", "Y", "Z"][*axis];
            let size = if *sign { 14. } else { 10. };
            let class = if *sign {
                format!("view-cap cap-{}", ["x", "y", "z"][*axis])
            } else {
                format!("view-cap cap-negative cap-neg-{}", ["x", "y", "z"][*axis])
            };
            out.push_str(&format!("<button id='{id}' class='{class}' style='position:absolute;left:{}px;top:{}px;width:{size}px;height:{size}px' label='{}{label} view' description='Look along {}{label}' action='Dispatch'><text id='{id}-label' text='{}' /></button>",p[0]-size*0.5,p[1]-size*0.5,if *sign {label} else {""},if *sign {"+"} else {"−"},if *sign {"−"} else {"+"}));
        }
        out.push_str("</panel>");
    }

    /// Update the widget without rebuilding or reshaping the surrounding panels.
    pub fn orient_axis(&mut self, camera: crate::state::Camera) {
        if self.axis_camera == camera || self.current.is_some() {
            return;
        }
        self.axis_camera = camera;
        let Some(root) = self.ui.find("view-cube") else {
            return;
        };
        let origin = self.ui.nodes[root].rect;
        for (axis, sign, _, p) in caps(&camera) {
            if let Some(i) = self.ui.find(&cap_id(axis, sign)) {
                let size = if sign { 14. } else { 10. };
                let rect = self.ui.nodes[i].rect;
                self.ui.nodes[i].paint_offset = [
                    self.ui.nodes[i].paint_offset[0] + origin.x + p[0] - size * 0.5 - rect.x,
                    self.ui.nodes[i].paint_offset[1] + origin.y + p[1] - size * 0.5 - rect.y,
                ];
                self.ui.nodes[i].dirty.paint = true;
            }
        }
    }

    pub(super) fn paint_axis(&mut self) {
        let Some(i) = self.ui.find("view-cube") else {
            return;
        };
        // Stems draw inside the widget's paint transform: use its layout position.
        let root = &self.ui.nodes[i];
        let x = root.rect.x - root.paint_offset[0];
        let y = root.rect.y - root.paint_offset[1];
        let mut stems = Vec::new();
        for (axis, sign, _, p) in caps(&self.axis_camera) {
            if !sign {
                continue;
            }
            let name = [
                "--destructive-foreground",
                "--success-foreground",
                "--info-foreground",
            ][axis];
            let color = self
                .ui
                .sheet
                .color_token(name)
                .unwrap_or(Color(160, 160, 160, 255));
            stems.push(Draw::Line {
                node: stable_id("axis-stem") + axis as u64,
                from: [x + SIZE * 0.5, y + SIZE * 0.5],
                to: [x + p[0], y + p[1]],
                width: 1.5,
                color,
            });
        }
        // Stems sit under the caps. Insert them before the centre button, which has no
        // paint transform; the caps carry per-camera offsets.
        let centre = stable_id("view-iso");
        let at = self
            .ui
            .display
            .commands
            .iter()
            .position(|d| matches!(d, Draw::Rect { node, .. } | Draw::RoundedRect { node, .. } | Draw::Border { node, .. } if *node == centre))
            .unwrap_or(self.ui.display.commands.len());
        self.ui.display.commands.splice(at..at, stems);
    }
}
fn cap_id(axis: usize, positive: bool) -> String {
    format!(
        "view-{}{}",
        ["x", "y", "z"][axis],
        if positive { "" } else { "-negative" }
    )
}
/// (axis, positive, depth toward the camera, widget-local position).
fn caps(camera: &crate::state::Camera) -> Vec<(usize, bool, f32, [f32; 2])> {
    let right = Vec3::new(camera.yaw.cos(), 0., -camera.yaw.sin());
    let forward = (camera.target - camera.position()).normalize();
    let up = right.cross(forward);
    (0..3)
        .flat_map(|axis| {
            [false, true].map(move |sign| {
                let v = [Vec3::X, Vec3::Y, Vec3::Z][axis] * if sign { 1. } else { -1. };
                (
                    axis,
                    sign,
                    -v.dot(forward),
                    [
                        SIZE * 0.5 + v.dot(right) * RADIUS,
                        SIZE * 0.5 - v.dot(up) * RADIUS,
                    ],
                )
            })
        })
        .collect()
}
