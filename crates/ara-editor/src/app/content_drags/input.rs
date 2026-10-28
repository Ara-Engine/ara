use super::*;
use crate::shell::Binding;

enum Find<'a> {
    Entity(&'a str),
    Asset(&'a str),
    Control(&'a str),
    Viewport,
}
fn centre(rect: Box2) -> [f32; 2] {
    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]
}
fn bounds(shell: &Shell, find: Find<'_>) -> Option<Box2> {
    match find {
        Find::Control(id) => shell.shown_node(id).map(|i| shell.ui.nodes[i].rect),
        Find::Viewport => shell.viewport(),
        Find::Entity(entity) => shell.bindings.iter().find_map(|(id, binding)| {
            if !matches!(binding, Binding::Select(id) if id == entity) {
                return None;
            }
            let i = shell.ui.by_id(*id)?;
            shell.ui.shown(i).then_some(shell.ui.nodes[i].rect)
        }),
        Find::Asset(path) => {
            let storage = fixture::storage(shell, path)?;
            shell.bindings.iter().find_map(|(id, binding)| {
                if !matches!(binding, Binding::Asset(asset) if asset.path == storage) {
                    return None;
                }
                let i = shell.ui.by_id(*id)?;
                shell.ui.shown(i).then_some(shell.ui.nodes[i].rect)
            })
        }
    }
}
fn motion(point: Point) -> (u32, Event) {
    (
        point.sdl,
        Event::MouseMotion {
            position: point.local.map(|v| v / point.scale),
            delta: [0.; 2],
        },
    )
}
fn button(sdl: u32, pressed: bool) -> (u32, Event) {
    (sdl, Event::MouseButton { button: 1, pressed })
}

impl ContentDrags {
    fn setup(&mut self, act: Act, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        let Some(folder) = fixture::storage(shell, "assets").map(str::to_owned) else {
            return self.wait_for_control();
        };
        shell.leave();
        editor.action("editor.layout.reset", json!({}))?;
        editor.action("editor.layout.show", json!({"panel":"assets"}))?;
        editor.action("editor.workspace", json!({"folder":folder}))?;
        let mut commands: Vec<_> = IDS
            .iter()
            .enumerate()
            .map(|(index, id)| json!({"cmd":"entity.reparent","args":{"entity":id,"index":index}}))
            .collect();
        if matches!(act, Act::Cycle) {
            commands
                .push(json!({"cmd":"entity.reparent","args":{"entity":IDS[1],"parent":IDS[0]}}));
        }
        editor.apply(commands, "Prepare behaviour fixture hierarchy", None)?;
        if let Act::Hierarchy(action) = act {
            hierarchy::prepare(action, editor)?;
        }
        let selection = match act {
            Act::MultiReorder => vec![IDS[2], IDS[1]],
            Act::EntityReference | Act::FloatingReference => vec![CAMERA],
            Act::Cycle => vec![IDS[0]],
            Act::Hierarchy(hierarchy::Action::Before) => vec![IDS[1], IDS[2]],
            Act::Hierarchy(_) => vec![IDS[1], IDS[0], IDS[2]],
            _ => vec![IDS[1]],
        };
        editor.action("editor.select", json!({"entities":selection}))?;
        if matches!(act, Act::FloatingReference) {
            editor.action(
                "editor.layout.move",
                json!({"panel":"inspector","slot":"float","rect":[1600,100,540,1400]}),
            )?;
        }
        shell.rebuild(editor)?;
        if matches!(act, Act::EntityReference) {
            shell.reveal_field("FollowCamera.target")?;
            shell.rebuild(editor)?;
        }
        self.before_revision = editor.host.store().revision();
        if matches!(act, Act::Hierarchy(_) | Act::Cycle) {
            self.hierarchy_proof = json!({"before":hierarchy::snapshot(editor)?});
        }
        self.before_entities = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?
            .into_keys()
            .collect();
        Ok(())
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "native surface owners and target geometry are borrowed independently"
    )]
    fn point(
        &self,
        find: Find<'_>,
        floating: bool,
        shell: &mut Shell,
        editor: &Editor,
        window: &Window,
        floats: &windows::Floats,
        main_scale: f32,
        fraction: f32,
    ) -> ara_ui::Result<Option<Point>> {
        let (rect, scale, sdl) = if floating {
            let Some(float) = floats.windows.iter().find(|float| {
                editor.state.floating.iter().any(|state| {
                    state.id == float.id
                        && super::super::panels_of(&state.root)
                            .iter()
                            .any(|panel| panel == "inspector")
                })
            }) else {
                return Ok(None);
            };
            if !shell.enter(&float.id) {
                return Ok(None);
            }
            (
                float.rect,
                crate::native::points_to_logical(
                    float.window.raw(),
                    float.window.display_scale() * windows::ui_scale(editor),
                ),
                float.sdl,
            )
        } else {
            shell.leave();
            (window.rect()?, main_scale, 0)
        };
        let result = bounds(shell, find).map(|bounds| {
            let mut local = centre(bounds);
            local[1] = bounds.y + bounds.h * fraction;
            Point {
                local,
                desktop: [
                    rect[0] as f32 + local[0] / scale,
                    rect[1] as f32 + local[1] / scale,
                ],
                scale,
                sdl,
            }
        });
        shell.leave();
        Ok(result)
    }
    fn wait_for_control(&mut self) -> ara_ui::Result<()> {
        if self.waited >= PATIENCE {
            return Err("The required fixture control or import did not become reachable within 120 seconds of frames".into());
        }
        self.waiting = true;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn input(
        &mut self,
        act: Act,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        floats: &windows::Floats,
        scale: f32,
        scene: &viewport::SceneView,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.age == 0 {
            return self.setup(act, editor, shell);
        }
        if matches!(act, Act::Hierarchy(_)) && [FINISH + 1, FINISH + 4].contains(&self.age) {
            for (scancode, pressed) in [
                (224, true),
                (if self.age == FINISH + 1 { 29 } else { 28 }, true),
                (if self.age == FINISH + 1 { 29 } else { 28 }, false),
                (224, false),
            ] {
                events.push((
                    window.id(),
                    Event::Key {
                        scancode,
                        pressed,
                        repeat: false,
                    },
                ));
            }
            return Ok(());
        }
        if self.age == BEGIN && matches!(act, Act::AssetSurface | Act::MaterialSurface) {
            let Some(point) = self.point(
                Find::Viewport,
                false,
                shell,
                editor,
                window,
                floats,
                scale,
                0.6,
            )?
            else {
                return self.wait_for_control();
            };
            let Some(rect) = shell.viewport() else {
                return self.wait_for_control();
            };
            if scene
                .surface_hit(&editor.state.camera, rect, point.local)
                .is_none()
            {
                return self.wait_for_control();
            }
        }
        if matches!(act, Act::OsImport) {
            if self.age == BEGIN {
                events.push((
                    0,
                    Event::FileDrop {
                        path: self.output.join("sources/os-source.png"),
                        position: [32., 32.],
                    },
                ));
            } else if self.age == FINISH
                && (fixture::storage(shell, "assets/os-source.png").is_none()
                    || editor
                        .host
                        .store()
                        .workspace()
                        .index()
                        .id_of("assets/os-source.png")
                        .is_none())
            {
                self.wait_for_control()?;
            }
            return Ok(());
        }
        if self.age == BEGIN {
            let find = match act {
                Act::AssetReference => Find::Asset("assets/rock.glb"),
                Act::InvalidAsset | Act::AssetSurface => Find::Asset("assets/box.prefab"),
                Act::MaterialSurface => Find::Asset("assets/coating.graph"),
                Act::EntityReference | Act::FloatingReference => Find::Entity(IDS[2]),
                Act::Cycle => Find::Entity(IDS[0]),
                Act::Hierarchy(hierarchy::Action::Before) => Find::Entity(IDS[1]),
                Act::Hierarchy(_) => Find::Entity(IDS[0]),
                Act::Component => Find::Control("collapse-component-MeshRenderer"),
                _ => Find::Entity(IDS[1]),
            };
            let Some(point) = self.point(find, false, shell, editor, window, floats, scale, 0.5)?
            else {
                return self.wait_for_control();
            };
            if matches!(act, Act::AssetReference)
                && editor
                    .host
                    .store()
                    .workspace()
                    .index()
                    .id_of("assets/rock.glb")
                    .is_none()
            {
                return self.wait_for_control();
            }
            self.source = Some(point);
            shell.desktop_pointer = Some(point.desktop);
            events.push(motion(point));
        } else if self.age == BEGIN + 1 {
            let point = self.source.ok_or("Missing source press")?;
            events.push(button(point.sdl, true));
        } else if self.age == BEGIN + 2 {
            let mut point = self.source.ok_or("Missing source drag")?;
            point.local[0] += 12.;
            point.desktop[0] += 12. / point.scale;
            shell.desktop_pointer = Some(point.desktop);
            events.push(motion(point));
        } else if self.age == BEGIN + 3 {
            let (find, floating, fraction) = match act {
                Act::MultiReorder => (Find::Entity(IDS[3]), false, 0.96),
                Act::Reparent => (Find::Entity(IDS[0]), false, 0.5),
                Act::Cycle => (Find::Entity(IDS[1]), false, 0.5),
                Act::Cancel => (Find::Entity(IDS[3]), false, 0.5),
                Act::AssetReference | Act::InvalidAsset => {
                    (Find::Control("asset-field-MeshRenderer-model"), false, 0.5)
                }
                Act::AssetSurface | Act::MaterialSurface => (Find::Viewport, false, 0.6),
                Act::EntityReference => (Find::Control("edit-FollowCamera-target"), false, 0.5),
                Act::FloatingReference => (Find::Control("edit-FollowCamera-target"), true, 0.5),
                Act::Component => (Find::Control("component-Transform"), false, 0.05),
                Act::OsImport => return Ok(()),
                Act::Hierarchy(hierarchy::Action::Before) => (Find::Entity(IDS[0]), false, 0.04),
                Act::Hierarchy(hierarchy::Action::Inside) => (Find::Entity(IDS[3]), false, 0.5),
                Act::Hierarchy(hierarchy::Action::After) => (Find::Entity(IDS[3]), false, 0.96),
                Act::Hierarchy(hierarchy::Action::Root) => {
                    (Find::Control("hierarchy-rows"), false, 0.96)
                }
            };
            let Some(point) = self.point(
                find, floating, shell, editor, window, floats, scale, fraction,
            )?
            else {
                return self.wait_for_control();
            };
            shell.desktop_pointer = Some(point.desktop);
            events.push(motion(point));
        } else if self.age == RELEASE {
            let source = self.source.ok_or("Missing source release")?;
            events.push(if matches!(act, Act::Cancel) {
                (
                    source.sdl,
                    Event::Key {
                        scancode: 41,
                        pressed: true,
                        repeat: false,
                    },
                )
            } else {
                button(source.sdl, false)
            });
        } else if self.age == RELEASE + 1 && matches!(act, Act::Cancel) {
            let source = self.source.ok_or("Missing cancelled source")?;
            events.push(button(source.sdl, false));
            events.push((
                source.sdl,
                Event::Key {
                    scancode: 41,
                    pressed: false,
                    repeat: false,
                },
            ));
        }
        Ok(())
    }
}
