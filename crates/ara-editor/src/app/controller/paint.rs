//! Mesh Paint strokes in the Scene (`crate::paint`): a press on a selected entity's
//! mesh paints (or fills, floods or picks), a drag dabs along the surface, release
//! commits one `vertexPaint.set` per changed primitive as one undoable transaction.
//! Until then the Scene shows the stroke as transient paint (`Editor::paint_preview`).
use super::*;
use crate::paint::{self as mesh_paint, Layer, PaintTool};
use std::collections::BTreeMap;

/// A stroke in progress: the painted layers (with their entity) and the last dab.
pub(super) struct Stroke {
    entities: Vec<String>,
    layers: Vec<Layer>,
    last: Vec3,
}

impl Controller {
    /// The selected entities' primitives as the Scene draws them, seeded with their
    /// current paint (or imported colours).
    fn paint_layers(
        editor: &Editor,
        view: &crate::world::DocumentView,
        scene: &viewport::SceneView,
    ) -> (Vec<String>, Vec<Layer>) {
        let mut entities = Vec::new();
        let mut layers = Vec::new();
        for address in &editor.state.selection {
            let Some(row) = view.entities().get(address) else {
                continue;
            };
            let Some(renderer) = row.record.get("MeshRenderer").and_then(|v| {
                serde_json::from_value::<ara_runtime::components::MeshRenderer>(v.clone()).ok()
            }) else {
                continue;
            };
            let paint: ara_runtime::paint::VertexPaint = row
                .record
                .get("VertexPaint")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            for (draw, model, index, world) in scene.paint_targets(address) {
                let base = if renderer.node.is_none() && renderer.primitive.is_none() {
                    0
                } else {
                    model
                        .select(renderer.node.as_deref(), renderer.primitive)
                        .map_or(0, |s| s.base)
                };
                let slot = index.saturating_sub(base) as u32;
                let existing = paint.get(slot).and_then(|p| p.decode().ok());
                let mut layer = Layer {
                    draw,
                    slot,
                    model,
                    index,
                    world,
                    colors: Vec::new(),
                    changed: false,
                };
                layer.seed(existing.as_deref());
                entities.push(address.to_owned());
                layers.push(layer);
            }
        }
        (entities, layers)
    }

    /// The world ray under the pointer, if it is over the Scene.
    fn paint_ray(editor: &Editor, shell: &Shell, size: [u32; 2]) -> Option<(Vec3, Vec3)> {
        let rect = shell.viewport()?;
        let ndc = [
            (shell.pointer[0] - rect.x) / rect.w * 2. - 1.,
            1. - (shell.pointer[1] - rect.y) / rect.h * 2.,
        ];
        editor.state.camera.ray(size, ndc)
    }

    /// A press in Mesh Paint mode. False when it hits none of the selected meshes,
    /// so the press selects as usual.
    pub(super) fn begin_paint(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        view: &crate::world::DocumentView,
        scene: &viewport::SceneView,
        size: [u32; 2],
    ) -> ara_ui::Result<bool> {
        let Some(settings) = editor.state.paint else {
            return Ok(false);
        };
        let (entities, mut layers) = Self::paint_layers(editor, view, scene);
        let Some((origin, direction)) = Self::paint_ray(editor, shell, size) else {
            return Ok(false);
        };
        let Some(hit) = mesh_paint::hit(&layers, origin, direction) else {
            return Ok(false);
        };
        match settings.tool {
            PaintTool::Brush | PaintTool::Erase => {
                let target = if settings.tool == PaintTool::Brush {
                    settings.color
                } else {
                    settings.erase
                };
                mesh_paint::dab(&mut layers, hit.point, &settings, target);
                self.paint = Some(Stroke {
                    entities,
                    layers,
                    last: hit.point,
                });
                self.preview_paint(editor);
            }
            PaintTool::Fill => {
                let picked = entities[hit.layer].clone();
                let keep: Vec<_> = entities.iter().map(|e| *e == picked).collect();
                let (entities, mut layers) = retain(entities, layers, &keep);
                mesh_paint::fill(&mut layers, settings.color, settings.channels);
                commit(editor, &entities, &layers, "Fill vertex colours")?;
            }
            PaintTool::Flood => {
                mesh_paint::flood(&mut layers, &hit, settings.color, settings.channels);
                commit(editor, &entities, &layers, "Flood vertex colours")?;
            }
            PaintTool::Pick => {
                let color = mesh_paint::pick(&layers, &hit);
                if let Some(paint) = &mut editor.state.paint {
                    paint.color = color;
                }
            }
        }
        Ok(true)
    }

    /// A drag of the current stroke: dabs where the brush moved a quarter radius.
    pub(super) fn drive_paint(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        size: [u32; 2],
    ) -> ara_ui::Result<bool> {
        let (Some(settings), Some(stroke)) = (editor.state.paint, &mut self.paint) else {
            return Ok(false);
        };
        let Some((origin, direction)) = Self::paint_ray(editor, shell, size) else {
            return Ok(false);
        };
        let Some(hit) = mesh_paint::hit(&stroke.layers, origin, direction) else {
            return Ok(false);
        };
        let spacing = (settings.radius * 0.25).max(1e-4);
        let distance = hit.point.distance(stroke.last);
        if distance < spacing {
            return Ok(false);
        }
        let target = if settings.tool == PaintTool::Erase {
            settings.erase
        } else {
            settings.color
        };
        // Dab evenly along the path so fast drags leave no gaps.
        let steps = (distance / spacing).floor().max(1.) as usize;
        let mut changed = false;
        for step in 1..=steps {
            let at = stroke.last.lerp(hit.point, step as f32 / steps as f32);
            changed |= mesh_paint::dab(&mut stroke.layers, at, &settings, target);
        }
        stroke.last = hit.point;
        if changed {
            self.preview_paint(editor);
        }
        Ok(changed)
    }

    /// Ends the stroke: `commit` writes its changed primitives as one transaction;
    /// otherwise (Escape, focus loss) the Scene drops the preview.
    pub(super) fn finish_paint(&mut self, editor: &mut Editor, commit: bool) -> ara_ui::Result<()> {
        let Some(stroke) = self.paint.take() else {
            return Ok(());
        };
        if commit {
            self::commit(
                editor,
                &stroke.entities,
                &stroke.layers,
                "Paint vertex colours",
            )?;
        }
        editor.paint_preview = None;
        editor.paint_revision = editor.paint_revision.wrapping_add(1);
        Ok(())
    }

    fn preview_paint(&self, editor: &mut Editor) {
        let Some(stroke) = &self.paint else {
            return;
        };
        let preview: BTreeMap<u64, std::sync::Arc<[u32]>> = stroke
            .layers
            .iter()
            .filter(|l| l.changed)
            .map(|l| (l.draw, l.cooked()))
            .collect();
        editor.paint_preview = Some(std::sync::Arc::new(preview));
        editor.paint_revision = editor.paint_revision.wrapping_add(1);
    }
}

fn retain(entities: Vec<String>, layers: Vec<Layer>, keep: &[bool]) -> (Vec<String>, Vec<Layer>) {
    entities
        .into_iter()
        .zip(layers)
        .zip(keep)
        .filter(|(_, keep)| **keep)
        .map(|(pair, _)| pair)
        .unzip()
}

/// One `vertexPaint.set` per changed layer, in one undoable transaction.
fn commit(
    editor: &mut Editor,
    entities: &[String],
    layers: &[Layer],
    why: &str,
) -> ara_ui::Result<()> {
    let commands: Vec<Value> = entities
        .iter()
        .zip(layers)
        .filter(|(_, l)| l.changed)
        .map(|(entity, layer)| {
            let colors = layer.quantized();
            json!({"cmd": "vertexPaint.set", "args": {
                "entity": entity,
                "primitive": layer.slot,
                "vertices": colors.len(),
                "colors": ara_runtime::paint::encode(&colors),
            }})
        })
        .collect();
    if !commands.is_empty() {
        editor.apply(commands, why, None)?;
    }
    Ok(())
}
