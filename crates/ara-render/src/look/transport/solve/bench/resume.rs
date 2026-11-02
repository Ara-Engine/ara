//! Reconstruct deterministic solver state from F0 after an interrupted capture.
//! Retained frame measurements stay intact; critical and join images are checked
//! again before a combined full receipt can be published.
use super::{FrameScore, Path};
use anyhow::{Result, ensure};

pub(super) struct Prefix {
    source: std::path::PathBuf,
    scores: Vec<FrameScore>,
}

impl Prefix {
    pub fn load(source: &Path, output: &Path, case: &str, frames: u64) -> Result<Self> {
        ensure!(
            source.file_name().and_then(|s| s.to_str()) == Some(case),
            "resume case directory differs from selected recipe"
        );
        ensure!(
            source.canonicalize()? != output.canonicalize()?,
            "resume output must be a new directory"
        );
        let scores: Vec<FrameScore> =
            serde_json::from_slice(&std::fs::read(source.join("frames.json"))?)?;
        Self::validate(&scores, frames)?;
        println!(
            "G3 RESUME reconstructing F0..F{}; preserving recorded measurements, rechecking critical and last three images",
            scores.len()
        );
        Ok(Self {
            source: source.to_owned(),
            scores,
        })
    }

    fn validate(scores: &[FrameScore], frames: u64) -> Result<()> {
        ensure!(
            !scores.is_empty() && (scores.len() as u64) < frames,
            "resume prefix must be nonempty and shorter than the recipe"
        );
        for (i, score) in scores.iter().enumerate() {
            ensure!(
                score.frame == i as u64 + 1,
                "resume prefix is not a contiguous F1 sequence"
            );
            ensure!(
                score.capture_repeat_valid == Some(true),
                "resume prefix has an invalid or missing capture repeat"
            );
            for metric in [
                score.region_flip_excess,
                score.exact_geometry_region_flip,
                score.exact_geometry_max_error_white.map(f64::from),
            ] {
                ensure!(
                    metric.is_some_and(|v| v.is_finite() && v >= 0.),
                    "resume prefix has a missing or invalid image metric"
                );
            }
        }
        Ok(())
    }

    pub fn get(&self, frame: u64) -> Option<&FrameScore> {
        frame
            .checked_sub(1)
            .and_then(|i| self.scores.get(i as usize))
    }

    pub fn capture(&self, frame: u64, event: u64) -> bool {
        frame == 1
            || (event..=event + 3).contains(&frame)
            || frame.saturating_add(3) > self.scores.len() as u64
    }

    pub fn verify(&self, actual: &FrameScore, image: bool) -> Result<()> {
        let Some(previous) = self.get(actual.frame) else {
            return Ok(());
        };
        ensure!(
            previous.max_error_white == actual.max_error_white
                && previous.mean_error_white == actual.mean_error_white,
            "resume patch solution differs at F{}",
            actual.frame
        );
        for (old, new) in [
            (&previous.uncapped, &actual.uncapped),
            (&previous.capped, &actual.capped),
        ] {
            ensure!(
                old.patch_passes == new.patch_passes
                    && old.repair_debt == new.repair_debt
                    && old.oldest_debt == new.oldest_debt
                    && old.missed_deadlines == new.missed_deadlines
                    && old.solution_bound == new.solution_bound
                    && old.topology_publication == new.topology_publication,
                "resume deterministic work/debt differs at F{}",
                actual.frame
            );
        }
        if image {
            ensure!(
                actual.capture_repeat_valid == Some(true),
                "resume capture repeat failed at F{}",
                actual.frame
            );
            ensure!(
                previous.region_flip_excess == actual.region_flip_excess
                    && previous.exact_geometry_max_error_white
                        == actual.exact_geometry_max_error_white
                    && previous.exact_geometry_region_flip == actual.exact_geometry_region_flip,
                "resume image metrics differ at F{}: previous {:?}/{:?}/{:?}, current {:?}/{:?}/{:?}",
                actual.frame,
                previous.region_flip_excess,
                previous.exact_geometry_max_error_white,
                previous.exact_geometry_region_flip,
                actual.region_flip_excess,
                actual.exact_geometry_max_error_white,
                actual.exact_geometry_region_flip
            );
            println!("G3 RESUME verified image overlap F{}", actual.frame);
        }
        Ok(())
    }

    pub fn receipt(&self) -> serde_json::Value {
        serde_json::json!({"source": self.source, "retained_frames": self.scores.len(),
            "state_reconstruction":"All capped and uncapped events replayed from F0; every retained patch error and deterministic work/debt field matched exactly",
            "image_overlap":"F1, F8-F11, and final three retained frames rechecked exactly, including identical-input capture repeat",
            "cold_measurement":"Cold timing and allocation peak measured again during reconstruction; earlier interrupted cold record was not flushed",
            "timing":"Retained prefix keeps its original CPU observations; replay reconstruction cost is excluded from the completed-case event distribution"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    fn score(frame: u64) -> FrameScore {
        FrameScore {
            frame,
            uncapped: Default::default(),
            capped: Default::default(),
            max_error_white: 0.,
            mean_error_white: 0.,
            region_flip_excess: Some(0.),
            exact_geometry_max_error_white: Some(0.),
            exact_geometry_region_flip: Some(0.),
            capture_repeat_valid: Some(true),
            capture_repeat_max_white: Some(0.),
            capture_repeat_region_flip: Some(0.),
            measured_transport_heap_live: 0,
            measured_transport_heap_peak: 0,
            frame_wall_ms: None,
        }
    }

    #[test]
    fn recovery_requires_contiguous_valid_receipts_and_exact_overlap() -> Result<()> {
        ensure!(Prefix::validate(&[score(2)], 4).is_err());
        let mut invalid = score(1);
        invalid.capture_repeat_valid = Some(false);
        ensure!(Prefix::validate(&[invalid], 4).is_err());
        let prefix = Prefix {
            source: "old".into(),
            scores: (1..=20).map(score).collect(),
        };
        Prefix::validate(&prefix.scores, 21)?;
        for frame in [1, 8, 9, 10, 11, 18, 19, 20, 21] {
            ensure!(prefix.capture(frame, 8));
        }
        for frame in [2, 7, 12, 17] {
            ensure!(!prefix.capture(frame, 8));
        }
        let mut actual = score(20);
        actual.uncapped.event_cpu_ms = 123.;
        prefix.verify(&actual, true)?;
        actual.capped.oldest_debt = 1;
        ensure!(prefix.verify(&actual, true).is_err());
        actual.capped.oldest_debt = 0;
        actual.exact_geometry_max_error_white = Some(0.01);
        ensure!(prefix.verify(&actual, true).is_err());
        ensure!(prefix.get(0).is_none());
        ensure!(prefix.get(21).is_none());
        ensure!(prefix.get(20).context("retained frame")?.frame == 20);
        Ok(())
    }
}
