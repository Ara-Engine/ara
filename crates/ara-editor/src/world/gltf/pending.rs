//! Read-only import status controls loading bounds, never source discovery.
use super::*;
use ara_play_host::ImportProgress;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Default, Clone)]
pub(super) struct Progress {
    pub revision: u64,
    pub value: Option<Arc<ImportProgress>>,
    pending: BTreeSet<String>,
    arrivals: BTreeSet<String>,
}

pub(super) fn style(
    placeholders: &mut ara_runtime_render::ModelPlaceholders,
    frame: &mut ara_runtime::extract::RenderFrame,
    entities: &ara_doc::prefab::Resolved,
    source: &dyn AssetSource,
    progress: Option<&ImportProgress>,
    whole: bool,
) -> bool {
    let models = entities
        .iter()
        .filter_map(|(address, row)| {
            let reference: ara_schema::AssetRef =
                serde_json::from_value(row.record.get("MeshRenderer")?.get("model")?.to_owned())
                    .ok()?;
            Some((
                address.to_owned(),
                ara_runtime::assets::asset_id(&reference).ok()?.to_owned(),
            ))
        })
        .collect();
    placeholders.style(
        frame,
        &models,
        |asset| {
            source.digest(asset).is_none()
                && progress
                    .is_none_or(|p| p.pending.contains(asset) && !p.failed.contains_key(asset))
        },
        whole,
    )
}

impl FidelityContent {
    /// The progress epoch is separate: changes patch placeholder factors without
    /// making decoded-model or geometry publication revisions advance.
    pub(crate) fn progress_pending(&self) -> bool {
        self.progress.revision != self.reader.progress_revision()
    }
    pub(super) fn refresh_progress(&mut self) -> bool {
        let revision = self.reader.progress_revision();
        if self.progress.revision == revision {
            return false;
        }
        // A Ready progress event can race adoption of its generation snapshot.
        // Retain the old status while that snapshot is busy, avoiding a pink flash.
        if self.snapshot.as_ref().map_or(0, |s| s.revision()) != self.reader.revision() {
            return false;
        }
        if let Some(progress) = self.reader.progress() {
            for asset in &self.progress.pending {
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.digest(asset).is_some())
                {
                    self.progress.arrivals.insert(asset.to_owned());
                }
            }
            self.progress.pending = progress
                .pending
                .iter()
                .filter(|asset| {
                    !progress.failed.contains_key(*asset)
                        && self
                            .snapshot
                            .as_ref()
                            .is_none_or(|s| s.digest(asset).is_none())
                })
                .map(String::to_owned)
                .collect();
            self.progress.value = Some(progress);
            self.progress.revision = revision;
            true
        } else {
            false
        }
    }
    pub(crate) fn style_pending(
        &mut self,
        frame: &mut ara_runtime::extract::RenderFrame,
        view: &crate::world::DocumentView,
    ) -> Option<bool> {
        use crate::world::EditWorldView;
        if !self.refresh_progress() {
            return None;
        }
        let empty = ara_runtime::assets::MemoryAssets::default();
        let source: &dyn AssetSource = self
            .snapshot
            .as_deref()
            .map_or(&empty as &dyn AssetSource, |s| s);
        Some(style(
            &mut self.placeholders,
            frame,
            view.entities(),
            source,
            self.progress.value.as_deref(),
            true,
        ))
    }
    pub(crate) fn model_is_pending(&self, asset: &str) -> bool {
        self.snapshot
            .as_ref()
            .is_none_or(|s| s.digest(asset).is_none())
            && self
                .progress
                .value
                .as_deref()
                .is_none_or(|p| p.pending.contains(asset) && !p.failed.contains_key(asset))
    }
    pub(crate) fn take_arrivals(&mut self) -> BTreeSet<String> {
        let mut arrivals = self.placeholders.take_arrivals();
        arrivals.append(&mut self.progress.arrivals);
        arrivals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::EditWorldView;
    #[test]
    fn progress_changes_placeholder_factors_without_a_model_publication() -> anyhow::Result<()> {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/renderer-unification/pending-status")
            .join(std::process::id().to_string());
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels"))?;
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("models"))?;
        for (path, value) in [
            (
                "ara.project.json",
                serde_json::json!({"name":"Pending style","layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                serde_json::json!({"$schema":"ara://schema/scene/1","id":"sc_pending01","records":{
                    "e_pending01":{"Transform":{},"MeshRenderer":{"model":"@models/model.gltf#as_pending1"}},
                    "e_missing01":{"Transform":{},"MeshRenderer":{"model":"@missing.gltf#as_missing1"}},
                    "e_authored01":{"Transform":{},"MeshRenderer":{}}
                }}),
            ),
            (
                "models/model.gltf.import.json",
                serde_json::json!({"$schema":"ara://schema/import/1","id":"as_pending1","records":{"i_settings":{"importer":"ara.gltf","version":"1","settings":{}}}}),
            ),
        ] {
            std::fs::write(
                if path == "ara.project.json" {
                    root.join(path)
                } else {
                    root.join(ara_doc::AUTHORING_ROOT).join(path)
                },
                ara_json::to_canonical_string(&value),
            )?;
        }
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("models/model.gltf"),
            "{}",
        )?;
        let editor = crate::Editor::open(&root).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut owner = ara_play_host::assets::Assets::new(&root, &root.join("absent-worker.exe"))
            .map_err(anyhow::Error::msg)?;
        let view = editor.view().map_err(|e| anyhow::anyhow!("{e}"))?;
        assert_eq!(view.entities().len(), 3);
        let mut content = FidelityContent::new(&root)?;
        let mut frame = content.frame(
            &view,
            editor.host.store().workspace(),
            editor.host.store().schema(),
            None,
            &Default::default(),
        )?;
        let color = |frame: &ara_runtime::extract::RenderFrame, address: &str| {
            frame
                .draws
                .iter()
                .find(|d| d.address == address)
                .map(|d| d.color)
        };
        assert_eq!(
            color(&frame, "e_pending01"),
            Some(ara_runtime_render::PENDING_MODEL_COLOR)
        );
        assert_eq!(color(&frame, "e_missing01"), Some([1., 0., 1., 1.]));
        let resolved = content.content(
            &frame,
            editor.host.store().workspace(),
            editor.host.store().schema(),
        )?;
        assert_eq!(
            resolved
                .unpublished_models
                .get("e_pending01")
                .map(String::as_str),
            Some("as_pending1")
        );
        assert_eq!(
            resolved
                .unpublished_models
                .get("e_missing01")
                .map(String::as_str),
            Some("as_missing1")
        );
        assert_eq!(
            resolved.unpublished_models.len(),
            2,
            "authored shapes have no model source"
        );
        let prepared = ara_runtime_render::fidelity::native::prepare(
            &frame,
            resolved,
            ara_runtime_render::fidelity::native::NativeConfig {
                resident_bytes: 1 << 20,
                upload_bytes: 1 << 20,
            },
        )?;
        for (draw, instance) in frame.draws.iter().zip(&prepared.scene.instances) {
            assert_eq!(
                prepared.scene.materials[instance.material].pbr.flags[1]
                    & ara_render::scene::MODEL_BOUNDS
                    != 0,
                draw.address != "e_authored01",
                "editor fallback metadata must reach the native corner-mark material"
            );
        }
        let revision = content.revision();
        let allocation = frame.draws.as_ptr();
        let start = std::time::Instant::now();
        while !content
            .reader
            .progress()
            .is_some_and(|p| p.failed.contains_key("as_pending1"))
        {
            anyhow::ensure!(
                start.elapsed().as_secs() < 20,
                "failed import status did not arrive"
            );
            owner.poll(false, true);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(content.style_pending(&mut frame, &view), Some(true));
        assert_eq!(color(&frame, "e_pending01"), Some([1., 0., 1., 1.]));
        assert_eq!(content.revision(), revision);
        assert_eq!(frame.draws.as_ptr(), allocation);
        assert_eq!(content.style_pending(&mut frame, &view), None);
        Ok(())
    }
}
