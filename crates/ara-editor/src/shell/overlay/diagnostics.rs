//! Read-only authored/retained geometry diagnostics, cached independently of rendering.
mod bounds;
mod collider;
mod projection;
mod stats;
#[cfg(test)]
mod tests;
mod volume;
use crate::{
    gpu::Overlay,
    state::{Camera, EditorState},
    world::{DocumentView, EditWorldView},
};
use ara_ui::{paint::Box2, style::Sheet};
use glam::Vec3;
use serde_json::{Value, json};
pub(crate) use stats::StatsSample;
use std::time::Duration;

/// Current authoring identity plus adopted native content, not a pending replacement.
/// Advance pose_revision for every adopted instance/skin pose that changes bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SourceKey {
    pub document_revision: u64,
    pub content_epoch: u64,
    pub publication_revision: u64,
    pub pose_revision: Option<u64>,
    pub scene: u64,
}
/// Borrowed native scene and its matching extraction provenance. No files/GPU reads.
#[derive(Clone, Copy)]
pub(crate) struct Source<'a> {
    pub scene: Option<&'a ara_render::instanced::Scene>,
    pub frame: Option<&'a ara_runtime::extract::RenderFrame>,
    pub key: SourceKey,
    /// False while the retained content belongs to a previous authoring input.
    pub current: bool,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Flags {
    bounds: bool,
    colliders: bool,
    stats: bool,
    volumes: bool,
}
impl Flags {
    fn read(state: &EditorState) -> Self {
        let on = |key| {
            state
                .settings
                .get(key)
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        let visible = !state.game_view && !state.hide_overlays;
        Self {
            bounds: visible && on("viewport.bounds"),
            colliders: visible && on("viewport.colliders"),
            stats: visible && on("viewport.stats"),
            volumes: visible && !state.hide_gizmos,
        }
    }
}
#[derive(Clone, Copy)]
enum Tint {
    Bounds,
    Collider,
    Sensor,
    Volume,
}
#[derive(Clone, Copy)]
struct Segment {
    from: Vec3,
    to: Vec3,
    tint: Tint,
}
#[derive(Clone, Copy)]
struct Line {
    from: [f32; 2],
    to: [f32; 2],
    tint: Tint,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Counts {
    mesh_bounds: usize,
    colliders: usize,
    sensors: usize,
    unsupported_colliders: usize,
    invalid_colliders: usize,
    pending_models: usize,
    unavailable_bounds: usize,
    invalid_bounds: usize,
    skipped_lines: usize,
    volumes: usize,
}
struct GeometryKey {
    source: SourceKey,
    doc: String,
    flags: Flags,
    current: bool,
    selection: u64,
}
struct ProjectionKey {
    camera: Camera,
    rect: [f32; 4],
}

/// Cached world wires and projected segments; unchanged UI frames do no extraction.
#[derive(Default)]
pub(crate) struct Diagnostics {
    geometry_key: Option<GeometryKey>,
    projection_key: Option<ProjectionKey>,
    wires: Vec<Segment>,
    lines: Vec<Line>,
    counts: Counts,
    flags: Flags,
    geometry_builds: u64,
    projection_builds: u64,
    markup_revision: u64,
    volumes: Vec<(String, f32)>,
    stats: stats::Stats,
}
impl Diagnostics {
    /// Append cached line instances. Returns true only when retained UI notices change.
    #[allow(
        clippy::too_many_arguments,
        reason = "borrows independent geometry, camera, retained content and theme owners"
    )]
    pub fn append(
        &mut self,
        overlays: &mut Vec<Overlay>,
        view: &DocumentView,
        state: &EditorState,
        rect: Box2,
        source: Source<'_>,
        theme: &Sheet,
    ) -> ara_ui::Result<bool> {
        let flags = Flags::read(state);
        let selection = if flags.volumes {
            state
                .selection
                .iter()
                .fold(0xcbf2_9ce4_8422_2325u64, |hash, id| {
                    (hash ^ crate::world::render_id(id)).wrapping_mul(0x0000_0100_0000_01b3)
                })
        } else {
            0
        };
        let previous_rect = self.projection_key.as_ref().map(|key| key.rect);
        let geometry_changed = self.geometry_key.as_ref().is_none_or(|key| {
            key.source != source.key
                || key.doc != state.doc
                || key.flags != flags
                || key.current != source.current
                || key.selection != selection
        });
        let mut markup_changed = self.flags != flags;
        self.flags = flags;
        if geometry_changed {
            let previous = self.counts;
            self.wires.clear();
            self.volumes.clear();
            self.counts = Default::default();
            if flags.bounds {
                bounds::collect(&mut self.wires, &mut self.counts, view, source);
            }
            let transforms = flags
                .colliders
                .then(|| crate::world::world_transforms(view.entities()));
            if let Some(transforms) = &transforms {
                collider::collect_with(&mut self.wires, &mut self.counts, view, state, transforms);
            }
            if flags.volumes {
                volume::collect(
                    &mut self.wires,
                    &mut self.volumes,
                    &mut self.counts,
                    view,
                    state,
                    transforms.as_ref(),
                );
            }
            self.geometry_key = Some(GeometryKey {
                source: source.key,
                doc: state.doc.clone(),
                flags,
                current: source.current,
                selection,
            });
            self.projection_key = None;
            self.geometry_builds += 1;
            markup_changed |= previous != self.counts;
        }
        let extent = [rect.x, rect.y, rect.w, rect.h];
        markup_changed |=
            previous_rect != Some(extent) && (flags.stats || flags.bounds || flags.colliders);
        if self
            .projection_key
            .as_ref()
            .is_none_or(|key| key.camera != state.camera || key.rect != extent)
        {
            self.lines.clear();
            self.counts.skipped_lines = 0;
            projection::project(
                &self.wires,
                &mut self.lines,
                &mut self.counts,
                &state.camera,
                rect,
            );
            self.projection_key = Some(ProjectionKey {
                camera: state.camera,
                rect: extent,
            });
            self.projection_builds += 1;
        }
        let color = |token: &str| -> ara_ui::Result<[f32; 4]> {
            let c = theme
                .color_token(token)
                .ok_or_else(|| format!("Missing diagnostic overlay token {token}"))?;
            Ok([
                f32::from(c.0) / 255.,
                f32::from(c.1) / 255.,
                f32::from(c.2) / 255.,
                0.9,
            ])
        };
        if !self.lines.is_empty() {
            let info = color("--info-foreground")?;
            // World volume cages use cyan, derived from the information palette.
            let cyan = [info[0].min(info[1]) * 0.5, info[2], info[2], info[3]];
            let tints = [
                info,
                color("--success-foreground")?,
                color("--warning-foreground")?,
                cyan,
            ];
            for line in &self.lines {
                if matches!(line.tint, Tint::Volume) {
                    overlays.push(Overlay::Line {
                        from: line.from,
                        to: line.to,
                        width: 1.5,
                        color: [0., 0., 0., 0.5],
                    });
                }
                overlays.push(Overlay::Line {
                    from: line.from,
                    to: line.to,
                    width: 0.75,
                    color: tints[line.tint as usize],
                });
            }
        }
        if markup_changed {
            self.markup_revision += 1;
        }
        Ok(markup_changed)
    }
    /// Lazily read completed metrics at most four times per second. No frame JSON.
    pub fn observe_stats(
        &mut self,
        elapsed: Duration,
        sample: impl FnOnce() -> StatsSample,
    ) -> bool {
        self.stats.observe(elapsed, self.flags.stats, sample)
    }
    /// Noninteractive retained UI; no display-list or GPU-specific text primitive.
    pub fn stats_markup(&self, state: &EditorState, out: &mut String) {
        self.stats.markup(
            Flags::read(state),
            &self.counts,
            self.geometry_key.as_ref().is_some_and(|key| key.current),
            self.projection_key
                .as_ref()
                .map(|key| [key.rect[2], key.rect[3]]),
            out,
        );
    }
    pub fn report(&self) -> Value {
        json!({"geometryBuilds":self.geometry_builds,"projectionBuilds":self.projection_builds,"markupRevision":self.markup_revision,
            "worldSegments":self.wires.len(),"projectedSegments":self.lines.len(),"counts":self.counts,
            "nativeSourceCurrent":self.geometry_key.as_ref().map(|key|key.current),"stats":self.stats.report(),
            "selectedVolumes":self.volumes.iter().map(|(entity,blend)|json!({"entity":entity,"blendDistance":blend})).collect::<Vec<_>>(),
            "scope":"Native transformed mesh AABBs, authored primitive physics shapes and selected local post-process cages; depth-unoccluded diagnostic lines, not contact/runtime physics debug geometry"})
    }
}
