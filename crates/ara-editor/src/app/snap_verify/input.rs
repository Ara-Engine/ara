use super::super::input_verify::native;
use super::*;
use std::collections::BTreeSet;

fn source(shell: &Shell, id: u64) -> bool {
    shell
        .outlines
        .channels
        .target_of(id)
        .is_some_and(|address| address == SOURCE || address == CHILD)
}

impl SnapCheck {
    pub(super) fn acquire(
        &mut self,
        act: Act,
        editor: &Editor,
        shell: &Shell,
        scene: &viewport::SceneView,
    ) -> ara_ui::Result<bool> {
        if !scene.snap_ready(&editor.state.doc) {
            return Ok(self.unready(
                "presented-scene-document-not-ready",
                json!({"doc":editor.state.doc}),
            ));
        }
        let Some((host, rect)) = shell.scene_host() else {
            return Ok(self.unready("scene-host-missing", Value::Null));
        };
        let ui = host
            .and_then(|id| shell.surface(id))
            .map_or(&shell.ui, |surface| &surface.ui);
        let vertices = scene.snap_vertices(|id| source(shell, id));
        let targets =
            scene.snap_vertices(|id| shell.outlines.channels.target_of(id) == Some(TARGET));
        if vertices.is_empty() || targets.is_empty() {
            return Ok(self.unready("selected-or-target-native-vertices-missing",json!({"sourceVertices":vertices.len(),"targetVertices":targets.len(),"viewport":rect})));
        }
        let excluded: BTreeSet<_> = vertices.iter().map(|vertex| vertex.entity).collect();
        self.vertices = vertices.iter().map(|vertex| vertex.position).collect();
        let view = editor.view()?;
        let pivot = crate::world::world_transform(view.entities(), SOURCE)
            .ok_or("Snap proof source missing")?
            .w_axis
            .truncate();
        if act.vertex() {
            let Some((from, vertex)) = vertices.iter().find_map(|vertex| {
                let point = overlay::project(&editor.state.camera, rect, vertex.position)?;
                if !rect.contains(point[0], point[1]) || ui.hit_test(point[0], point[1]).is_some() {
                    return None;
                }
                let picked =
                    scene.snap_source_vertex(&editor.state.camera, rect, point, 18., &excluded)?;
                (picked.position.distance(vertex.position) < 1e-4)
                    .then_some((point, picked.position))
            }) else {
                return Ok(self.unready("source-vertex-not-visibly-acquirable",json!({"sourceVertices":vertices.len(),"targetVertices":targets.len(),"viewport":rect})));
            };
            let Some((to, target)) = targets.iter().find_map(|vertex| {
                let point = overlay::project(&editor.state.camera, rect, vertex.position)?;
                if !rect.contains(point[0], point[1]) || ui.hit_test(point[0], point[1]).is_some() {
                    return None;
                }
                let picked =
                    scene.snap_vertex(&editor.state.camera, rect, point, 18., &excluded)?;
                (shell.outlines.channels.target_of(picked.entity) == Some(TARGET))
                    .then_some((point, picked.position))
            }) else {
                return Ok(self.unready("target-vertex-not-visibly-acquirable",json!({"sourceVertices":vertices.len(),"targetVertices":targets.len(),"viewport":rect})));
            };
            self.from = from;
            self.to = to;
            self.source = Some(vertex);
            self.target = Some(target);
            self.delta = target - vertex;
        } else if act.drop() {
            let low = self
                .vertices
                .iter()
                .copied()
                .reduce(Vec3::min)
                .ok_or("Snap source geometry missing")?;
            let high = self
                .vertices
                .iter()
                .copied()
                .reduce(Vec3::max)
                .ok_or("Snap source geometry missing")?;
            let origin = Vec3::new(
                (low.x + high.x) * 0.5,
                high.y + 0.001,
                (low.z + high.z) * 0.5,
            );
            let Some(hit) = scene.ray_hit_excluding(origin, -Vec3::Y, &excluded) else {
                return Ok(self.unready("vertical-drop-has-no-native-triangle-hit",json!({"rayOrigin":origin.to_array(),"sourceVertices":vertices.len(),"targetVertices":targets.len()})));
            };
            self.target = Some(Vec3::from(hit.position));
            self.normal = Some(if hit.normal[1] < 0. {
                -Vec3::from(hit.normal)
            } else {
                Vec3::from(hit.normal)
            });
        } else {
            let handles = overlay::handles(&view, &editor.state, rect)
                .ok_or("Snap proof has no Translate handles")?;
            let arm = glam::Vec2::from(handles.ends[0]) - glam::Vec2::from(handles.origin);
            self.from = (glam::Vec2::from(handles.origin) + arm * 0.65).to_array();
            let picked_handle = overlay::hit(&handles, self.from);
            if picked_handle != Some(overlay::Handle::Axis(0)) {
                return Ok(self.unready("source-x-handle-pick-differs",json!({"from":self.from,"origin":handles.origin,"ends":handles.ends,"picked":format!("{picked_handle:?}"),"viewport":rect})));
            }
            if !rect.contains(self.from[0], self.from[1])
                || ui.hit_test(self.from[0], self.from[1]).is_some()
            {
                return Ok(self.unready("source-x-handle-obstructed-by-ui",json!({"from":self.from,"viewport":rect,
                    "hit":ui.hit_test(self.from[0],self.from[1]).map(|index|ui.nodes[index].element.id.as_str())})));
            }
            self.to = overlay::project(&editor.state.camera, rect, Vec3::new(4., 3., 0.))
                .ok_or("Snap target outside camera")?;
            let Some(hit) =
                scene.surface_hit_excluding(&editor.state.camera, rect, self.to, &excluded)
            else {
                return Ok(self.unready("target-pointer-has-no-native-triangle-hit",json!({"to":self.to,"viewport":rect,"sourceVertices":vertices.len(),"targetVertices":targets.len()})));
            };
            if shell.outlines.channels.target_of(hit.entity) != Some(TARGET) {
                return Ok(self.unready("target-pointer-hit-another-entity",json!({"to":self.to,"hitEntity":hit.entity,
                    "hitAddress":shell.outlines.channels.target_of(hit.entity),"hitPosition":hit.position,"viewport":rect})));
            }
            let mut normal = Vec3::from(hit.normal);
            let (_, direction) = editor
                .state
                .camera
                .ray(
                    [rect.w as u32, rect.h as u32],
                    [
                        (self.to[0] - rect.x) / rect.w * 2. - 1.,
                        1. - (self.to[1] - rect.y) / rect.h * 2.,
                    ],
                )
                .ok_or("Snap proof camera ray missing")?;
            if normal.dot(direction) > 0. {
                normal = -normal;
            }
            let support = self
                .vertices
                .iter()
                .map(|vertex| (*vertex - pivot).dot(normal))
                .min_by(f32::total_cmp)
                .ok_or("Snap proof support missing")?;
            self.delta = Vec3::from(hit.position) - pivot + normal * (0.125 - support);
            self.target = Some(Vec3::from(hit.position));
            self.normal = Some(normal);
            if act == Act::Invalid {
                self.to = [rect.x - 100., rect.y - 100.];
            }
        }
        self.readiness = json!({"reason":"ready","sourceVertices":vertices.len(),"targetVertices":targets.len(),"viewport":rect,"from":self.from,"to":self.to});
        Ok(true)
    }
    fn unready(&mut self, reason: &str, detail: Value) -> bool {
        self.readiness = json!({"reason":reason,"detail":detail});
        false
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        main: &Window,
        floats: &windows::Floats,
        main_scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        shell.leave();
        let (host, _) = shell
            .scene_host()
            .ok_or("Snap proof Scene host disappeared")?;
        let (window, scale) = if let Some(id) = host {
            let float = floats
                .windows
                .iter()
                .find(|float| float.id == id)
                .ok_or("Snap proof floating native window did not open")?;
            (
                &float.window,
                crate::native::points_to_logical(
                    float.window.raw(),
                    float.window.display_scale().max(0.25) * windows::ui_scale(editor),
                ),
            )
        } else {
            (main, main_scale)
        };
        if self.age == 7 {
            native::motion(window, self.from, scale, events)?;
            if act.vertex() {
                native::key(window, 25, true, events)?;
            }
        } else if self.age == 8 {
            if act.drop() {
                native::tap(window, 77, events)?;
            } else {
                native::button(window, true, events)?;
            }
        } else if !act.drop() && (9..=11).contains(&self.age) {
            let t = (self.age - 8) as f32 / 3.;
            native::motion(
                window,
                [
                    self.from[0] + (self.to[0] - self.from[0]) * t,
                    self.from[1] + (self.to[1] - self.from[1]) * t,
                ],
                scale,
                events,
            )?;
        } else if self.age == 13 {
            if act == Act::Escape {
                native::tap(window, 41, events)?;
            }
            if act == Act::DeletedTarget {
                editor.apply(
                    vec![json!({"cmd":"entity.delete","args":{"entities":[TARGET]}})],
                    "Delete snap target during native preview",
                    None,
                )?;
            }
        } else if self.age == 14 && !act.drop() {
            native::button(window, false, events)?;
            if act.vertex() {
                native::key(window, 25, false, events)?;
            }
        } else if self.age == 21 && act.commits() {
            native::shortcut(window, 29, events)?;
        } else if self.age == 25 && act.commits() {
            native::shortcut(window, 28, events)?;
        }
        Ok(())
    }
}
