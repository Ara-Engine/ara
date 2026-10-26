//! A rejected generation retries finitely; interactive polls never rebuild it.
use super::content::Key;
use std::time::{Duration, Instant};

const ATTEMPTS: u8 = 3;

pub(super) enum Admission<'a> {
    Ready,
    Wait(Duration),
    Exhausted(&'a str),
}

struct Failure {
    key: Key,
    attempts: u8,
    retry_at: Instant,
    error: String,
}

#[derive(Default)]
pub(super) struct Recovery {
    failure: Option<Failure>,
    target: Option<([u32; 2], Option<ara_runtime_render::QualityChoice>)>,
}

fn same_generation(a: Key, b: Key) -> bool {
    // Pose/extraction notifications cannot defeat backoff. A publication, real
    // edit, projected draft or scene change may fix the rejected generation.
    a.revision.0 == b.revision.0
        && a.revision.2 == b.revision.2
        && a.revision.3 == b.revision.3
        && a.epoch == b.epoch
        && a.scene == b.scene
}

impl Recovery {
    /// A size or preset change can fix allocation pressure without changing the
    /// document. Ordinary polls keep the same target and cannot restart retries.
    pub fn configure(
        &mut self,
        size: [u32; 2],
        quality: Option<ara_runtime_render::QualityChoice>,
    ) {
        let target = (size, quality);
        if self.target != Some(target) {
            self.target = Some(target);
            self.succeeded();
        }
    }
    pub fn admit(&mut self, key: Key, now: Instant) -> Admission<'_> {
        if self
            .failure
            .as_ref()
            .is_some_and(|f| !same_generation(f.key, key))
        {
            self.failure = None;
        }
        let Some(failure) = &self.failure else {
            return Admission::Ready;
        };
        if failure.attempts >= ATTEMPTS {
            Admission::Exhausted(&failure.error)
        } else if now < failure.retry_at {
            Admission::Wait(failure.retry_at - now)
        } else {
            Admission::Ready
        }
    }

    /// Return true once per rejected generation, for its Problems diagnostic.
    pub fn failed(&mut self, key: Key, now: Instant, error: String) -> bool {
        if self
            .failure
            .as_ref()
            .is_some_and(|f| same_generation(f.key, key) && f.attempts >= ATTEMPTS)
        {
            return false;
        }
        let attempts = self
            .failure
            .as_ref()
            .filter(|f| same_generation(f.key, key))
            .map_or(1, |f| f.attempts.saturating_add(1));
        self.failure = Some(Failure {
            key,
            attempts,
            error,
            retry_at: now + Duration::from_millis(if attempts == 1 { 250 } else { 1000 }),
        });
        attempts == 1
    }

    pub fn succeeded(&mut self) {
        self.failure = None;
    }
    pub fn pending(&self) -> bool {
        self.failure.as_ref().is_some_and(|f| f.attempts < ATTEMPTS)
    }
    /// Stop progressive wakeups after exhaustion. A newer publication or an
    /// explicit input request can still admit a different generation.
    pub fn blocked(&self, publication: u64) -> bool {
        self.failure
            .as_ref()
            .is_some_and(|f| f.attempts >= ATTEMPTS && f.key.revision.3 == publication)
    }
}

#[cfg(test)]
pub(super) fn exercise_pressure(
    scene: &mut super::SceneView,
    renderer: &mut ara_render::Renderer,
    editor: &crate::Editor,
    view: &crate::world::DocumentView,
) -> anyhow::Result<()> {
    use ara_gpu::{Device, Resource, vk};
    renderer.wait_scene_frame()?;
    renderer.gpu_mut().collect()?;
    let output = renderer.output();
    let capacity = renderer.gpu().context().info.storage_buffer_capacity as usize;
    let mut pressure = Vec::new();
    while renderer.gpu().descriptor_usage().storage_buffers < capacity {
        let gpu = renderer.gpu_mut();
        let buffer = gpu.create_buffer(16, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
        gpu.storage_buffer(buffer)?;
        pressure.push(buffer);
    }
    let usage = renderer.gpu().descriptor_usage();
    scene.set_preview_view(None);
    for attempt in 1..=ATTEMPTS {
        if let Some(failure) = &mut scene.recovery.failure {
            failure.retry_at = Instant::now();
        }
        let error = scene
            .render_capture(renderer, editor, view, 1)
            .expect_err("publication cannot fit under descriptor pressure");
        assert!(
            error.to_string().contains("Buffer heap exhausted"),
            "{error}"
        );
        assert_eq!(
            scene.recovery.failure.as_ref().expect("failure").attempts,
            attempt
        );
        assert!(scene.content_job.is_none());
        assert!(scene.content_ready.is_none());
        renderer.gpu_mut().collect()?;
        assert_eq!(renderer.gpu().descriptor_usage(), usage);
        assert_eq!(
            renderer.output(),
            output,
            "last-good image survives rejection"
        );
        scene.recovery.failure.as_mut().expect("failure").retry_at =
            Instant::now() + Duration::from_secs(60);
        // Hundreds of real preview/live polls cannot launch CPU preparation or
        // reserve another descriptor while the rejected generation backs off.
        for _ in 0..100 {
            scene
                .render_raster_inner(renderer, editor, view, super::RasterWork::Prepare)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            scene
                .render(renderer, editor, view, 1)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            assert!(scene.content_job.is_none());
            assert!(scene.content_ready.is_none());
        }
    }
    assert!(!scene.recovery.pending());
    assert!(!scene.preview_needs_frame());
    assert!(!scene.needs_frame(true), "exhaustion must not spin the UI");
    for buffer in pressure {
        renderer.gpu_mut().retire(Resource::Buffer(buffer))?;
    }
    renderer.gpu_mut().collect()?;
    assert!(
        scene.render_capture(renderer, editor, view, 1).is_err(),
        "an explicit request cannot defeat exhausted-generation recovery"
    );
    scene.set_preview_view(None);
    scene
        .render_capture(renderer, editor, view, 1)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(scene.recovery.failure.is_none());
    assert!(
        scene
            .raster
            .as_ref()
            .expect("raster")
            .has_prepared_content()
    );
    renderer.gpu().assert_clean()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Key {
        Key {
            revision: (1, 2, 3, 4),
            epoch: 5,
            pose: Some(6),
            scene: 7,
        }
    }

    #[test]
    fn retries_back_off_stop_and_report_only_once_despite_repeated_pose_polls() {
        let mut recovery = Recovery::default();
        let now = Instant::now();
        assert!(recovery.failed(key(), now, "heap full".into()));
        for frame in 0..200 {
            let current = Key {
                pose: Some(frame),
                revision: (1, frame, 3, 4),
                ..key()
            };
            assert!(matches!(recovery.admit(current, now), Admission::Wait(_)));
        }
        let later = now + Duration::from_millis(250);
        assert!(matches!(recovery.admit(key(), later), Admission::Ready));
        assert!(!recovery.failed(key(), later, "heap full".into()));
        assert!(matches!(recovery.admit(key(), later), Admission::Wait(_)));
        let later = later + Duration::from_secs(1);
        assert!(matches!(recovery.admit(key(), later), Admission::Ready));
        assert!(!recovery.failed(key(), later, "heap full".into()));
        assert!(!recovery.pending());
        assert!(recovery.blocked(4));
        assert!(
            !recovery.blocked(8),
            "a newer publication must wake recovery"
        );
        assert!(!recovery.failed(key(), later, "capture rejected".into()));
        assert!(matches!(
            recovery.admit(key(), later + Duration::from_secs(60)),
            Admission::Exhausted("heap full")
        ));
    }

    #[test]
    fn a_new_publication_or_input_supersedes_backoff_and_success_goes_idle() {
        let mut recovery = Recovery::default();
        let now = Instant::now();
        recovery.failed(key(), now, "heap full".into());
        let published = Key {
            revision: (1, 2, 3, 8),
            ..key()
        };
        assert!(matches!(recovery.admit(published, now), Admission::Ready));
        assert!(recovery.failed(published, now, "bad asset".into()));
        let edited = Key {
            revision: (2, 2, 3, 8),
            ..published
        };
        assert!(matches!(recovery.admit(edited, now), Admission::Ready));
        recovery.failed(edited, now, "bad asset".into());
        recovery.succeeded();
        assert!(!recovery.pending());
        assert!(matches!(recovery.admit(edited, now), Admission::Ready));
    }

    #[test]
    fn unchanged_targets_preserve_backoff_but_resize_and_quality_changes_can_recover() {
        use ara_runtime_render::{QualityChoice, QualityPreset};
        let mut recovery = Recovery::default();
        let now = Instant::now();
        let high = Some(QualityChoice::Preset(QualityPreset::High));
        recovery.configure([1920, 1080], high);
        recovery.failed(key(), now, "heap full".into());
        recovery.configure([1920, 1080], high);
        assert!(matches!(recovery.admit(key(), now), Admission::Wait(_)));
        recovery.configure([960, 540], high);
        assert!(matches!(recovery.admit(key(), now), Admission::Ready));
        recovery.failed(key(), now, "heap full".into());
        recovery.configure([960, 540], Some(QualityChoice::Preset(QualityPreset::Low)));
        assert!(matches!(recovery.admit(key(), now), Admission::Ready));
    }
}
