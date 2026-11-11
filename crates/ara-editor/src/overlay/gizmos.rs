//! Scene gizmos, as Unity draws them: a billboard icon for every iconed component
//! (lights, cameras, audio, probes, effects, volumes) and a wire shape for every
//! component with an extent (light ranges and cones, camera frusta, audio ranges,
//! reverb zones, colliders, character capsules, joints, navigation and perception).
//!
//! The selection draws full detail, the hovered icon's entity a lighter copy and every
//! other entity a subtle hint. `viewport.gizmos` hides them all, `viewport.gizmoTypes`
//! single types, `viewport.icons*` the icons (all from the Scene toolbar's Gizmos menu).
//! Icons are clickable ([`Gizmos::at`], before the ID buffer).
mod billboard;
mod parts;
mod shapes;
#[cfg(test)]
mod tests;
mod wire;

use super::*;
use glam::Mat4;
use parts::{Icon, Part};

/// The Gizmos menu's choices, read from editor settings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    /// Master switch (`viewport.gizmos`): component gizmos and icons.
    pub show: bool,
    /// Billboard icons (`viewport.icons`).
    pub icons: bool,
    /// Icons shrink with distance (`viewport.icons3d`), like Unity's 3D Icons.
    pub icons_3d: bool,
    /// Icon size factor (`viewport.iconSize` percent / 100).
    pub icon_scale: f32,
    /// Hidden types (`viewport.gizmoTypes` keys set to `"off"`).
    hidden: Vec<String>,
}
impl Settings {
    pub fn read(state: &EditorState) -> Self {
        let flag = |key: &str| {
            state
                .settings
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true)
        };
        let hidden = state
            .settings
            .get("viewport.gizmoTypes")
            .and_then(serde_json::Value::as_object)
            .map(|types| {
                types
                    .iter()
                    .filter(|(_, v)| v.as_str() == Some("off"))
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            show: flag("viewport.gizmos"),
            icons: flag("viewport.icons"),
            icons_3d: flag("viewport.icons3d"),
            icon_scale: (state.number("viewport.iconSize", 100.) as f32 / 100.).clamp(0.25, 3.),
            hidden,
        }
    }
    /// Whether gizmos of type `key` draw.
    pub fn shows(&self, key: &str) -> bool {
        !self.hidden.iter().any(|h| h == key)
    }
}

/// How much of a gizmo an entity draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Detail {
    /// Unselected: a faint hint (or nothing, for shapes Unity only draws selected).
    Subtle,
    /// Its icon is under the pointer.
    Hover,
    /// Selected: the full shape.
    Full,
}

/// One gizmo-bearing entity of the current view.
struct Source {
    entity: String,
    id: u64,
    world: Mat4,
    parts: Vec<Part>,
    icon: Option<Icon>,
}

/// Gizmo sources, cached per document view and visibility; projected every frame.
#[derive(Default)]
pub struct Gizmos {
    key: Option<(u64, usize, u64)>,
    sources: Vec<Source>,
    /// The icons last drawn, far to near: entity, logical centre and radius.
    drawn: Vec<(String, [f32; 2], f32)>,
}

impl Gizmos {
    /// Collect the view's gizmo entities again when the view or hidden set changed.
    fn refresh(&mut self, view: &DocumentView, state: &EditorState, revision: u64) {
        let hidden = state
            .hidden
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |hash, id| {
                (hash ^ crate::world::render_id(id)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        let key = (
            revision,
            std::ptr::from_ref(view.entities()) as usize,
            hidden,
        );
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        self.sources.clear();
        let transforms = crate::world::world_transforms(view.entities());
        for (id, entity) in view.entities() {
            if state.hidden.contains(id) {
                continue;
            }
            let (parts, icon) = parts::classify(&entity.record);
            if parts.is_empty() && icon.is_none() {
                continue;
            }
            let Some(world) = transforms.get(id).copied() else {
                continue;
            };
            self.sources.push(Source {
                entity: id.clone(),
                id: crate::world::render_id(id),
                world,
                parts,
                icon,
            });
        }
    }

    /// The entity whose icon is under `pointer`, nearest first.
    pub fn at(&self, pointer: [f32; 2]) -> Option<&str> {
        self.drawn
            .iter()
            .rev()
            .find(|(_, c, radius)| Vec2::from(pointer).distance(Vec2::from(*c)) <= radius + 2.)
            .map(|(entity, _, _)| entity.as_str())
    }

    /// Append every visible gizmo shape (subtle, then hovered, then selected), then the
    /// icons far to near, and remember where icons were drawn for [`at`](Self::at).
    /// A dragged selection draws at its previewed pose (`GizmoState::offset`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw(
        &mut self,
        overlays: &mut Vec<Overlay>,
        view: &DocumentView,
        state: &EditorState,
        rect: Box2,
        revision: u64,
        gizmo: GizmoState,
        color: &impl Fn(&str, f32) -> ara_ui::Result<[f32; 4]>,
    ) -> ara_ui::Result<()> {
        self.drawn.clear();
        let settings = Settings::read(state);
        if state.game_view || !settings.show {
            return Ok(());
        }
        self.refresh(view, state, revision);
        let Some(mut pen) = wire::Pen::new(&state.camera, rect) else {
            return Ok(());
        };
        let palette = shapes::Palette::new(color)?;
        let mut order: Vec<(Detail, usize, Mat4)> = Vec::with_capacity(self.sources.len());
        for (n, source) in self.sources.iter().enumerate() {
            let selected = state.selection.contains(&source.entity);
            let detail = if selected {
                Detail::Full
            } else if gizmo.icon == Some(source.id) {
                Detail::Hover
            } else {
                Detail::Subtle
            };
            let world = if selected {
                gizmo.offset * source.world
            } else {
                source.world
            };
            order.push((detail, n, world));
        }
        // The selection paints last, over every hint.
        order.sort_by_key(|(detail, n, _)| (*detail, *n));
        for (detail, n, world) in &order {
            for part in &self.sources[*n].parts {
                if settings.shows(part.kind) {
                    shapes::draw(&mut pen, part, *world, *detail, &palette);
                }
            }
            pen.flush(overlays);
        }
        if settings.icons {
            let icons = order.iter().filter_map(|(detail, n, world)| {
                let source = &self.sources[*n];
                let icon = source.icon.as_ref()?;
                settings
                    .shows(icon.kind)
                    .then_some((source, icon, *world, *detail))
            });
            billboard::draw(
                overlays,
                &mut self.drawn,
                &pen,
                &settings,
                &palette,
                icons.map(|(source, icon, world, detail)| billboard::Item {
                    entity: &source.entity,
                    icon,
                    position: world.transform_point3(Vec3::ZERO),
                    detail,
                }),
            );
        }
        Ok(())
    }
}
