//! Native value edits reuse the current frame and its immutable model generations.
use super::*;
use crate::world::{DocumentView, EditWorldView, Patch, gesture::Gesture};
use std::collections::BTreeSet;

impl FidelityContent {
    pub(crate) fn patch(
        &mut self,
        frame: &mut ara_runtime::extract::RenderFrame,
        view: &DocumentView,
        workspace: &ara_doc::Workspace,
        patch: &Patch,
        gesture: Option<&Gesture>,
        hidden: &BTreeSet<String>,
    ) -> anyhow::Result<bool> {
        let mut wanted = patch.moved.to_owned();
        wanted.extend(patch.renamed.iter().cloned());
        if patch.materials {
            wanted.extend(view.entities().iter().filter_map(|(id, row)| {
                row.record
                    .get("MeshRenderer")
                    .filter(|value| self.mesh_records.get(id) != Some(*value))
                    .map(|_| id.to_owned())
            }));
        }
        if patch.lights {
            wanted.extend(
                view.entities()
                    .iter()
                    .filter(|(_, row)| {
                        [
                            "Light",
                            "Environment",
                            "SkyAtmosphere",
                            "HeightFog",
                            "PostProcessVolume",
                        ]
                        .iter()
                        .any(|key| row.record.contains_key(*key))
                    })
                    .map(|(id, _)| id.to_owned()),
            );
        }
        if wanted.is_empty() && !patch.lights {
            return Ok(true);
        }
        if wanted.iter().any(|id| {
            view.entities().get(id).is_none_or(|row| {
                row.record.contains_key("Terrain") || row.record.contains_key("TerrainChunk")
            })
        }) {
            return Ok(false);
        }
        let transforms = transforms(view.entities(), &wanted, gesture)?;
        let empty = ara_runtime::assets::MemoryAssets::default();
        let source: &dyn AssetSource = self
            .snapshot
            .as_deref()
            .map_or(&empty as &dyn AssetSource, |snapshot| snapshot);
        let mut delta = ara_runtime_render::fidelity::native::document_frame_selected(
            view.entities(),
            &transforms,
            workspace,
            &mut self.library,
            source,
            None,
            hidden,
            Some(&wanted),
        )?;
        // A model/selector/publication layout change takes the replacement path.
        // Check everything before changing the retained frame.
        if delta.draws.iter().any(|draw| {
            frame
                .draws
                .iter()
                .find(|old| old.entity == draw.entity)
                .is_none_or(|old| {
                    old.skin.is_some()
                        || std::mem::discriminant(&old.geometry)
                            != std::mem::discriminant(&draw.geometry)
                })
        }) {
            return Ok(false);
        }
        super::pending::style(
            &mut self.placeholders,
            &mut delta,
            view.entities(),
            source,
            self.progress.value.as_deref(),
            false,
        );
        for draw in delta.draws.drain(..) {
            let slot = frame
                .draws
                .iter_mut()
                .find(|old| old.entity == draw.entity)
                .expect("patch identities were checked before adoption");
            *slot = draw;
        }
        if patch.lights {
            frame.lights = delta.lights;
            frame.sun = delta.sun;
            frame.look = delta.look;
            frame.environment = delta.environment;
        }
        for id in wanted {
            if let Some(value) = view
                .entities()
                .get(&id)
                .and_then(|row| row.record.get("MeshRenderer"))
            {
                self.mesh_records.insert(id, value.to_owned());
            }
        }
        Ok(true)
    }
}

/// Resolve just the requested parent chains, using the same local-pose and gizmo
/// override rules as `world::transforms_with`. Unrelated entities are untouched.
fn transforms(
    entities: &ara_doc::prefab::Resolved,
    wanted: &BTreeSet<String>,
    gesture: Option<&Gesture>,
) -> anyhow::Result<BTreeMap<String, glam::Mat4>> {
    let mut out = BTreeMap::new();
    for id in wanted {
        let mut chain = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = Some(id.as_str());
        while let Some(id) = current {
            if out.contains_key(id) {
                break;
            }
            anyhow::ensure!(seen.insert(id), "native patch parent cycle at {id}");
            let row = entities.get(id).context("native patch entity missing")?;
            chain.push(id);
            current = row.parent.as_deref();
        }
        while let Some(id) = chain.pop() {
            let row = &entities[id];
            let parent = row
                .parent
                .as_ref()
                .and_then(|id| out.get(id))
                .copied()
                .unwrap_or(glam::Mat4::IDENTITY);
            let local = gesture
                .and_then(|gesture| gesture.current.get(id))
                .copied()
                .unwrap_or_else(|| crate::world::local_transform(&row.record));
            out.insert(id.to_owned(), parent * local);
        }
    }
    Ok(out)
}
