//! Sparse host-phase evidence for whole-process hitches, including keepalive.
use super::{CpuFrame, Duration, Presenter, Value, json};
use serde::Serialize;

const DETAILS: usize = 256;

#[derive(Serialize)]
struct Hitch {
    frame: usize,
    elapsed_since_launch_ms: f64,
    collecting_gpu_window: bool,
    frame_ms: f64,
    host_wait_ms: Option<f64>,
    work_ms: Option<f64>,
    simulation_and_extraction_ms: f64,
    presentation_prepare_ms: f64,
    presentation_including_display_ms: f64,
    audio_ms: f64,
    gpu_span_ms: Option<f64>,
    gpu_submission_gap_ms: Option<f64>,
    readiness: ara_runtime_render::PresentationReadiness,
}

#[derive(Default)]
pub(super) struct Samples {
    frames: Vec<Hitch>,
    omitted: u64,
}

impl Samples {
    pub fn record(
        &mut self,
        index: usize,
        elapsed: Duration,
        collecting: bool,
        presenter: &Presenter,
        frame: &CpuFrame,
    ) {
        if frame.first_presented || frame.total_ms <= 16. {
            return;
        }
        if self.frames.len() == DETAILS {
            self.omitted += 1;
            return;
        }
        let wait = frame
            .wait_ms
            .filter(|v| v.is_finite() && *v >= 0. && *v <= frame.total_ms);
        let stats = presenter.stats();
        self.frames.push(Hitch {
            frame: index,
            elapsed_since_launch_ms: elapsed.as_secs_f64() * 1000.,
            collecting_gpu_window: collecting,
            frame_ms: frame.total_ms,
            host_wait_ms: wait,
            work_ms: wait.map(|v| frame.total_ms - v),
            simulation_and_extraction_ms: frame.phases[0],
            presentation_prepare_ms: frame.phases[1],
            presentation_including_display_ms: frame.phases[2],
            audio_ms: frame.phases[3],
            gpu_span_ms: stats.gpu_span_ms,
            gpu_submission_gap_ms: stats.gpu_submission_gap_ms,
            readiness: presenter.readiness(),
        });
    }

    pub fn report(&self) -> Value {
        json!({"frames":self.frames,"omitted":self.omitted})
    }
}
