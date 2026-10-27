//! Editor targets for the UI kit's typed drag session. Preview never changes authoring data.
use super::*;
use ara_ui::{
    drag_drop::{self as kit, DragSession, Feedback, Indicator, Placement, Pointer},
    paint::Draw,
};
#[path = "drag_drop_commit.rs"]
mod commit;
#[path = "drag_drop_hierarchy.rs"]
mod hierarchy;
#[path = "drag_drop_material.rs"]
mod material;
#[path = "drag_drop_paint.rs"]
mod paint;
#[path = "drag_drop_targets.rs"]
mod targets;

pub(crate) enum Payload {
    Entity(String),
    Asset(assets::Asset),
    Component(String),
    Collection(collection_adapter::Action),
}

#[cfg(test)]
#[path = "drag_drop_tests.rs"]
mod tests;
pub(crate) enum Target {
    Hierarchy {
        entity: Option<String>,
        placement: Placement,
    },
    AssetField(String),
    AssetFolder(String),
    EntityField(String),
    Animation {
        doc: String,
        item: String,
        key: String,
    },
    Viewport([f32; 3]),
    Component {
        name: String,
        after: bool,
    },
    Material {
        entity: String,
        primitive: u32,
    },
    Collection {
        row: collection_adapter::Action,
        after: bool,
    },
}
struct SurfacePlacement {
    surface: u64,
    pointer: [f32; 2],
    position: [f32; 3],
    normal: [f32; 3],
    vertex: [f32; 3],
}
struct MaterialPlacement {
    surface: u64,
    pointer: [f32; 2],
    entity: String,
    primitive: u32,
}
pub(crate) struct ContentDrag {
    session: DragSession<Payload, Target>,
    desktop_mapped: bool,
    surface_hit: Option<SurfacePlacement>,
    material_hit: Option<MaterialPlacement>,
}
impl ContentDrag {
    pub(super) fn begin(payload: Payload, shell: &Shell) -> Option<Self> {
        DragSession::begin(
            payload,
            Pointer {
                surface: surface_id(shell),
                local: shell.pointer,
                desktop: shell.pointer,
            },
        )
        .map(|session| Self {
            session,
            desktop_mapped: false,
            surface_hit: None,
            material_hit: None,
        })
    }
}
fn surface_id(shell: &Shell) -> u64 {
    shell.current.as_deref().map_or(0, stable_id)
}
fn reject(reason: impl Into<String>, indicator: Option<Indicator>) -> Feedback<Target> {
    Feedback::Reject {
        reason: reason.into(),
        indicator,
    }
}
fn accept(target: Target, indicator: Indicator, label: impl Into<String>) -> Feedback<Target> {
    Feedback::Accept {
        target,
        indicator,
        label: label.into(),
    }
}
const NO_GROUND: &str = "Point at a surface or the ground plane to place this asset";
const NO_MATERIAL_TARGET: &str = "Drop this material onto a rendered mesh";

impl Shell {
    /// A content source is held, including the click phase before the shared threshold.
    pub fn content_pending(&self) -> bool {
        matches!(self.drag, Some(Drag::Content(_)))
    }
    /// Content capture has crossed the shared movement threshold.
    pub fn content_dragging(&self) -> bool {
        matches!(&self.drag, Some(Drag::Content(drag)) if drag.session.active())
    }
    /// An asset drag is active and may need a viewport surface ray test.
    pub fn content_asset_dragging(&self) -> bool {
        matches!(&self.drag, Some(Drag::Content(drag)) if drag.session.active() && matches!(drag.session.payload(), Payload::Asset(_)))
    }
    /// Inspect a transient drag for scripted journeys and accessibility evidence.
    pub fn content_drag_report(&self) -> Value {
        let Some(Drag::Content(drag)) = &self.drag else {
            return Value::Null;
        };
        let source = match drag.session.payload() {
            Payload::Entity(id) => json!({"kind":"entity","id":id}),
            Payload::Asset(asset) => json!({"kind":"asset","path":asset.path}),
            Payload::Component(name) => json!({"kind":"component","name":name}),
            Payload::Collection(row) => {
                json!({"kind":"collection","field":row.field,"pointer":row.pointer})
            }
        };
        let feedback = match drag.session.feedback() {
            Some(Feedback::Accept {
                target,
                indicator,
                label,
            }) => {
                let target = match target {
                    Target::Hierarchy { entity, placement } => {
                        json!({"kind":"hierarchy","entity":entity,"placement":format!("{placement:?}")})
                    }
                    Target::AssetField(field) | Target::EntityField(field) => {
                        json!({"kind":"reference","field":field})
                    }
                    Target::AssetFolder(folder) => {
                        json!({"kind":"assetFolder","storagePath":folder})
                    }
                    Target::Animation { doc, item, key } => {
                        json!({"kind":"animation","doc":doc,"item":item,"key":key})
                    }
                    Target::Viewport(position) => json!({"kind":"viewport","position":position}),
                    Target::Component { name, after } => {
                        json!({"kind":"component","name":name,"after":after})
                    }
                    Target::Material { entity, primitive } => {
                        json!({"kind":"material","entity":entity,"primitive":primitive})
                    }
                    Target::Collection { row, after } => {
                        json!({"kind":"collection","field":row.field,"pointer":row.pointer,"after":after})
                    }
                };
                json!({"valid":true,"label":label,"rect":indicator.rect(),"target":target})
            }
            Some(Feedback::Reject { reason, indicator }) => {
                json!({"valid":false,"reason":reason,"rect":indicator.map(Indicator::rect)})
            }
            None => Value::Null,
        };
        json!({"source":source,"active":drag.session.active(),"surface":drag.session.pointer().surface,"pointer":drag.session.pointer().local,"feedback":feedback,"surfaceHit":drag.surface_hit.as_ref().map(|hit|hit.position),"surfaceNormal":drag.surface_hit.as_ref().map(|hit|hit.normal),"surfaceVertex":drag.surface_hit.as_ref().map(|hit|hit.vertex)})
    }
    /// Update content capture from a desktop point after entering its destination surface.
    /// Supply one consistent logical desktop coordinate system for every window.
    pub fn content_drag_pointer(&mut self, desktop: [f32; 2]) {
        let surface = surface_id(self);
        let Some(Drag::Content(drag)) = &mut self.drag else {
            return;
        };
        let previous = drag.session.pointer();
        if !drag.desktop_mapped {
            let delta = if previous.surface == surface {
                [
                    self.pointer[0] - previous.local[0],
                    self.pointer[1] - previous.local[1],
                ]
            } else {
                [0.; 2]
            };
            drag.session.translate_desktop([
                desktop[0] - previous.desktop[0] - delta[0],
                desktop[1] - previous.desktop[1] - delta[1],
            ]);
            drag.desktop_mapped = true;
        }
        drag.session.move_to(Pointer {
            surface,
            local: self.pointer,
            desktop,
        });
    }
    /// Releasing outside every application surface is a rejected drop, never an implicit action.
    pub fn content_drag_outside(&mut self) {
        if let Some(Drag::Content(drag)) = &mut self.drag {
            drag.session.preview(reject(
                "Drop onto a hierarchy, reference field or Scene",
                None,
            ));
        }
        self.invalidate_content_drag();
    }
    /// Re-resolve the target and edge scrolling against the live retained tree.
    /// Call on pointer movement and while a held drag needs scrolling without movement.
    pub fn content_drag_over(&mut self, editor: &Editor, seconds: f32) -> ara_ui::Result<bool> {
        let mut drag = match self.drag.take() {
            Some(Drag::Content(drag)) => drag,
            other => {
                self.drag = other;
                return Ok(false);
            }
        };
        let previous = drag.session.pointer();
        let surface = surface_id(self);
        let desktop = if previous.surface == surface {
            [
                previous.desktop[0] + self.pointer[0] - previous.local[0],
                previous.desktop[1] + self.pointer[1] - previous.local[1],
            ]
        } else {
            previous.desktop
        };
        if !drag.session.move_to(Pointer {
            surface,
            local: self.pointer,
            desktop,
        }) {
            self.drag = Some(Drag::Content(drag));
            return Ok(false);
        }
        let result = (|| -> ara_ui::Result<()> {
            for id in [
                "hierarchy-rows",
                "asset-rows",
                "asset-grid",
                "asset-folders",
                "inspector-scroll",
            ] {
                let Some(i) = self.shown_node(id) else {
                    continue;
                };
                let rect = self.ui.nodes[i].rect.intersection(self.ui.nodes[i].clip);
                let delta = kit::auto_scroll(rect, self.pointer, seconds);
                if delta != [0.; 2]
                    && let Some(offset) = self.ui.scroll_position(i)
                {
                    self.ui
                        .scroll_to(i, [offset[0] + delta[0], offset[1] + delta[1]])?;
                    self.sync_scroll_views()?;
                }
            }
            let mut feedback = self.content_target(editor, drag.session.payload())?;
            let surface_target = matches!(
                &feedback,
                Feedback::Accept {
                    target: Target::Viewport(_),
                    ..
                }
            ) || matches!(&feedback, Feedback::Reject { reason, .. } if reason == NO_GROUND);
            if let Some(hit) = &drag.surface_hit
                && surface_target
                && hit.surface == surface
                && hit.pointer == self.pointer
                && let Some(rect) = self
                    .viewport()
                    .filter(|rect| rect.contains(self.pointer[0], self.pointer[1]))
            {
                feedback = accept(
                    Target::Viewport(hit.position),
                    Indicator::Inside(rect),
                    "Place on surface",
                );
            }
            if matches!(&feedback, Feedback::Reject { reason, .. } if reason == NO_MATERIAL_TARGET)
                && let Some(hit) = &drag.material_hit
                && hit.surface == surface
                && hit.pointer == self.pointer
                && let Payload::Asset(asset) = drag.session.payload()
            {
                feedback =
                    self.material_drop_feedback(editor, asset, &hit.entity, hit.primitive)?;
            }
            drag.session.preview(feedback);
            Ok(())
        })();
        self.drag = Some(Drag::Content(drag));
        self.invalidate_content_drag();
        result.map(|()| true)
    }
    /// Replace a ground-plane fallback with a precise surface hit supplied by the viewport.
    pub fn content_surface_hit(&mut self, position: [f32; 3]) {
        self.content_surface_hit_details(position, [0., 1., 0.], position);
    }
    /// Publish a precise native hit and its normal/vertex candidate for placement evidence.
    pub fn content_surface_hit_details(
        &mut self,
        position: [f32; 3],
        normal: [f32; 3],
        vertex: [f32; 3],
    ) {
        if !position
            .into_iter()
            .chain(normal)
            .chain(vertex)
            .all(f32::is_finite)
        {
            return;
        }
        let Some(rect) = self.viewport() else {
            return;
        };
        let surface = surface_id(self);
        if let Some(Drag::Content(drag)) = &mut self.drag
            && drag.session.active()
            && rect.contains(self.pointer[0], self.pointer[1])
            && matches!(drag.session.payload(), Payload::Asset(asset) if matches!(asset.kind.as_str(), "prefab" | "glb" | "gltf" | "fbx"))
        {
            drag.surface_hit = Some(SurfacePlacement {
                surface,
                pointer: self.pointer,
                position,
                normal,
                vertex,
            });
            drag.session.preview(accept(
                Target::Viewport(position),
                Indicator::Inside(rect),
                "Place on surface",
            ));
        }
    }
}
