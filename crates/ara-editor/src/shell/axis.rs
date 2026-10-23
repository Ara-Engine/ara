//! Camera-oriented sphere gizmo. Caps remain real accessible UI buttons.
use super::*;
use ara_ui::{paint::Draw, source::stable_id, style::Color};
use glam::Vec3;

impl Shell {
    pub(super) fn axis_markup(&mut self, editor: &Editor, out: &mut String) {
        out.push_str("<panel id='view-cube' style='position:absolute;right:12px;top:12px;width:96px;height:96px'>");
        let camera = editor.state.camera;
        self.axis_camera = camera;
        let mut caps = caps(&camera);
        caps.sort_by(|a, b| a.2.total_cmp(&b.2));
        for front in [false, true] {
            if front {
                self.bindings.insert(stable_id("view-iso"), Binding::Protocol("editor.camera", json!({"target":camera.target.to_array(),"yaw":0.6,"pitch":0.4,"distance":camera.distance})));
                out.push_str("<button id='view-iso' class='view-sphere' style='position:absolute;left:36px;top:36px;width:24px;height:24px' label='Perspective view' description='Return to perspective view' action='Dispatch' />");
            }
            for (axis, sign, depth, p) in &caps {
                if (*depth >= 0.) != front {
                    continue;
                }
                let id = cap_id(*axis, *sign);
                let (yaw, pitch) = match (*axis, *sign) {
                    (0, true) => (std::f32::consts::FRAC_PI_2, 0.),
                    (0, false) => (-std::f32::consts::FRAC_PI_2, 0.),
                    (1, true) => (0., 1.54),
                    (1, false) => (0., -1.54),
                    (2, true) => (0., 0.),
                    _ => (std::f32::consts::PI, 0.),
                };
                self.bindings.insert(stable_id(&id), Binding::Protocol("editor.camera",json!({"target":camera.target.to_array(),"yaw":yaw,"pitch":pitch,"distance":camera.distance})));
                let label = ["X", "Y", "Z"][*axis];
                out.push_str(&format!("<button id='{id}' class='view-cap cap-{} {}' style='position:absolute;left:{}px;top:{}px;width:20px;height:20px' text='{}' label='{}{} view' description='Snap camera to {}{} axis' action='Dispatch' />", ["x","y","z"][*axis], if *sign {"cap-positive"} else {"cap-negative"},p[0]-10.,p[1]-10.,if *sign {label} else {""},if *sign {"+"}else{"−"},label,if *sign {"+"}else{"−"},label));
            }
        }
        out.push_str("</panel>");
    }

    /// Update the view gizmo without rebuilding or reshaping the surrounding panels.
    pub fn orient_axis(&mut self, camera: crate::state::Camera) {
        if self.axis_camera == camera {
            return;
        }
        self.axis_camera = camera;
        for (axis, sign, _, p) in caps(&camera) {
            if let Some(i) = self.ui.find(&cap_id(axis, sign)) {
                let Some(root) = self.ui.find("view-cube") else {
                    continue;
                };
                let origin = self.ui.nodes[root].rect;
                let rect = self.ui.nodes[i].rect;
                self.ui.nodes[i].paint_offset = [
                    origin.x + p[0] - 10. - rect.x,
                    origin.y + p[1] - 10. - rect.y,
                ];
                self.ui.nodes[i].dirty.paint = true;
            }
        }
    }

    pub(super) fn paint_axis(&mut self) {
        let Some(i) = self.ui.find("view-cube") else {
            return;
        };
        let root = &self.ui.nodes[i];
        let x = root.rect.x + root.paint_offset[0];
        let y = root.rect.y + root.paint_offset[1];
        let mut stems = Vec::new();
        for (axis, sign, _, p) in caps(&self.axis_camera) {
            let name = [
                "--destructive-foreground",
                "--success-foreground",
                "--info-foreground",
            ][axis];
            let mut color = self
                .ui
                .sheet
                .color_token(name)
                .unwrap_or(Color(160, 160, 160, 255));
            if !sign {
                color.3 = 90;
            }
            // Overlapping analytic circles form a smooth two-pixel stem at every DPI.
            for step in 0..24 {
                let t = step as f32 / 24.;
                stems.push(Draw::RoundedRect {
                    node: stable_id("axis-stem") + axis as u64 * 48 + u64::from(sign) * 24 + step,
                    rect: Box2 {
                        x: x + 48. + (p[0] - 48.) * t - 1.,
                        y: y + 48. + (p[1] - 48.) * t - 1.,
                        w: 2.,
                        h: 2.,
                    },
                    radius: 1.,
                    color,
                });
            }
        }
        let at = self.ui.display.commands.iter().position(|d| matches!(d, Draw::RoundedRect {node,..} | Draw::Rect {node,..} | Draw::Border {node,..} if *node == stable_id("view-iso") || (0..3).any(|a| [false,true].iter().any(|s| *node == stable_id(&cap_id(a,*s)))))).unwrap_or(self.ui.display.commands.len());
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
                    [48. + v.dot(right) * 32., 48. - v.dot(up) * 32.],
                )
            })
        })
        .collect()
}
