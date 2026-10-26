//! Bounded CPU content preparation: publication may advance, edits may not.
use crate::world::gltf::PreparedScene;
use std::thread::JoinHandle;

#[derive(Clone, Copy)]
pub(super) struct Key {
    pub revision: (u64, u64, u64, u64),
    pub epoch: u64,
    pub pose: Option<u64>,
    pub scene: u64,
}

impl Key {
    /// An extraction notification alone does not change content already shown.
    pub fn same_input(self, current: Self) -> bool {
        self.matches(current) && self.revision.3 == current.revision.3
    }

    pub fn matches(self, current: Self) -> bool {
        self.revision.0 == current.revision.0
            && self.revision.2 == current.revision.2
            && self.epoch == current.epoch
            && self.pose == current.pose
            && self.scene == current.scene
    }
}

pub(super) fn scene_key(editor: &crate::Editor) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::hash::DefaultHasher::new();
    editor.state.doc.hash(&mut hash);
    editor.state.hidden.hash(&mut hash);
    hash.finish()
}

pub(super) fn queue_reason(previous: Option<(u64, u64, u64, u64)>, current: Key) {
    match previous {
        None => {
            ara_gpu::cpu_scope!("SceneContent.request.invalidated");
        }
        Some(old) if old.0 != current.revision.0 => {
            ara_gpu::cpu_scope!("SceneContent.request.document");
        }
        Some(old) if old.1 != current.revision.1 => {
            ara_gpu::cpu_scope!("SceneContent.request.extraction");
        }
        Some(old) if old.2 != current.revision.2 => {
            ara_gpu::cpu_scope!("SceneContent.request.review");
        }
        Some(old) if old.3 != current.revision.3 => {
            ara_gpu::cpu_scope!("SceneContent.request.publication");
        }
        _ => {
            ara_gpu::cpu_scope!("SceneContent.request.pose");
        }
    }
}

pub(super) struct Job {
    pub key: Key,
    pub thread: JoinHandle<anyhow::Result<PreparedScene>>,
}

/// One CPU-complete candidate, retained while its native allocations are prepared.
pub(super) struct Publication {
    pub key: Key,
    pub scene: PreparedScene,
    pub ready: bool,
}

/// Poll only; live calls never join an unfinished extraction or wait for native
/// allocations. Superseded candidates cancel their resources before being dropped.
pub(super) fn poll(
    job: &mut Option<Job>,
    publication: &mut Option<Publication>,
    current: Key,
    renderer: &mut ara_render::Renderer,
    wait: bool,
) -> anyhow::Result<()> {
    if publication
        .as_ref()
        .is_some_and(|p| !p.key.matches(current))
    {
        publication
            .take()
            .expect("superseded publication")
            .scene
            .raster
            .destroy(renderer)?;
    }
    if job
        .as_ref()
        .is_some_and(|job| wait || job.thread.is_finished())
    {
        let job = job.take().expect("completed content job");
        let result = job
            .thread
            .join()
            .map_err(|_| anyhow::anyhow!("Scene content worker panicked"))?;
        if job.key.matches(current) {
            *publication = Some(Publication {
                key: job.key,
                scene: result?,
                ready: false,
            });
        }
    }
    if let Some(candidate) = publication {
        match candidate.scene.raster.preallocate(renderer) {
            Ok(ready) => candidate.ready = ready,
            Err(error) => {
                publication
                    .take()
                    .expect("failed publication")
                    .scene
                    .raster
                    .destroy(renderer)?;
                return Err(error);
            }
        }
    }
    Ok(())
}

impl Job {
    pub fn start(
        key: Key,
        content: &crate::world::gltf::FidelityContent,
        editor: &crate::Editor,
        view: &crate::world::DocumentView,
        renderer: &ara_render::Renderer,
    ) -> anyhow::Result<Self> {
        let store = editor.host.store();
        Ok(Self {
            key,
            thread: content.prepare_async(
                view,
                store.workspace(),
                store.schema(),
                editor.gesture.as_ref(),
                &editor.state.hidden,
                renderer.native_seed(),
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_notifications_do_not_starve_partial_publication_but_edits_supersede_it() {
        let key = Key {
            revision: (1, 2, 3, 4),
            epoch: 5,
            pose: Some(6),
            scene: 7,
        };
        // Editor::sync advances extraction_revision on every model publication.
        assert!(key.matches(Key {
            revision: (1, 200, 3, 400),
            ..key
        }));
        assert!(key.same_input(Key {
            revision: (1, 200, 3, 4),
            ..key
        }));
        assert!(!key.same_input(Key {
            revision: (1, 2, 3, 5),
            ..key
        }));
        assert!(!key.matches(Key {
            revision: (2, 2, 3, 4),
            ..key
        }));
        assert!(!key.matches(Key {
            revision: (1, 2, 4, 4),
            ..key
        }));
        assert!(!key.matches(Key { epoch: 6, ..key }));
        assert!(!key.matches(Key { pose: None, ..key }));
        assert!(!key.matches(Key { scene: 8, ..key }));
    }
}
