//! Detect requested GPU output starving while native input frames still progress.
use serde_json::{Value, json};
use std::{collections::VecDeque, time::Duration};

const RETAINED_INTERVALS: usize = 32;
const MIN_REQUESTED_SAMPLES: u64 = 2;

/// Scalars supplied by the real native output owner; absent samples remain unknown.
#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub requested: bool,
    pub submissions: u32,
    pub presentations: u32,
    pub offscreen: bool,
}

struct Waiting {
    first_frame: u64,
    started: Duration,
    requested_samples: u64,
    elapsed: Duration,
    stalled: bool,
}
struct Interval {
    first_requested_frame: u64,
    detected_frame: u64,
    started: Duration,
    detected: Duration,
    no_output_requested: Duration,
    offscreen: bool,
    ended_frame: Option<u64>,
    ended: Option<Duration>,
    reason: Option<&'static str>,
}

pub(super) struct Health {
    threshold: Duration,
    clock: Duration,
    frames: u64,
    native_samples: u64,
    unknown_samples: u64,
    requested_samples: u64,
    offscreen_samples: u64,
    submissions: u64,
    presentations: u64,
    stalls: u64,
    recovered: u64,
    idle_resets: u64,
    unknown_resets: u64,
    omitted: u64,
    longest_wait: Duration,
    waiting: Option<Waiting>,
    latest: &'static str,
    intervals: VecDeque<Interval>,
}
impl Health {
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            clock: Duration::ZERO,
            frames: 0,
            native_samples: 0,
            unknown_samples: 0,
            requested_samples: 0,
            offscreen_samples: 0,
            submissions: 0,
            presentations: 0,
            stalls: 0,
            recovered: 0,
            idle_resets: 0,
            unknown_resets: 0,
            omitted: 0,
            longest_wait: Duration::ZERO,
            waiting: None,
            latest: "unknown",
            intervals: VecDeque::new(),
        }
    }

    /// Explicit frame durations make this deterministic and independent of sleeping
    /// or test wall clocks. Normal observations mutate scalars without allocation.
    pub fn record(&mut self, elapsed: Duration, sample: Option<Sample>) {
        let before = self.clock;
        self.clock = self.clock.saturating_add(elapsed);
        self.frames += 1;
        let Some(sample) = sample else {
            self.unknown_samples += 1;
            self.close("unknown-sample", before);
            self.latest = "unknown";
            return;
        };
        self.native_samples += 1;
        self.offscreen_samples += u64::from(sample.offscreen);
        self.submissions += u64::from(sample.submissions);
        self.presentations += u64::from(sample.presentations);
        self.requested_samples += u64::from(sample.requested);
        // A real submission is recovery, even if dirty was cleared later in the
        // sample. Presenting an old image alone cannot satisfy requested new output.
        if sample.submissions > 0 {
            self.close("submission", self.clock);
            self.latest = "output";
            return;
        }
        if !sample.requested {
            self.close("not-requested", before);
            self.latest = "idle";
            return;
        }
        let waiting = self.waiting.get_or_insert(Waiting {
            first_frame: self.frames,
            started: before,
            requested_samples: 0,
            elapsed: Duration::ZERO,
            stalled: false,
        });
        waiting.elapsed = waiting.elapsed.saturating_add(elapsed);
        waiting.requested_samples += 1;
        self.longest_wait = self.longest_wait.max(waiting.elapsed);
        if !waiting.stalled
            && waiting.requested_samples >= MIN_REQUESTED_SAMPLES
            && waiting.elapsed >= self.threshold
        {
            waiting.stalled = true;
            self.stalls += 1;
            if self.intervals.len() == RETAINED_INTERVALS {
                self.intervals.pop_front();
                self.omitted += 1;
            }
            self.intervals.push_back(Interval {
                first_requested_frame: waiting.first_frame,
                detected_frame: self.frames,
                started: waiting.started,
                detected: self.clock,
                no_output_requested: waiting.elapsed,
                offscreen: sample.offscreen,
                ended_frame: None,
                ended: None,
                reason: None,
            });
        }
        if waiting.stalled {
            if let Some(interval) = self.intervals.back_mut() {
                interval.no_output_requested = waiting.elapsed;
            }
            self.latest = "stalled";
        } else {
            self.latest = "waiting";
        }
    }

    fn close(&mut self, reason: &'static str, ended: Duration) {
        let Some(waiting) = self.waiting.take().filter(|w| w.stalled) else {
            return;
        };
        match reason {
            "submission" => self.recovered += 1,
            "not-requested" => self.idle_resets += 1,
            _ => self.unknown_resets += 1,
        }
        if let Some(interval) = self.intervals.back_mut() {
            interval.no_output_requested = waiting.elapsed;
            interval.ended_frame = Some(self.frames);
            interval.ended = Some(ended);
            interval.reason = Some(reason);
        }
    }

    pub fn stalls(&self) -> u64 {
        self.stalls
    }
    pub fn report(&self) -> Value {
        let observed = self.native_samples >= MIN_REQUESTED_SAMPLES;
        let healthy = if self.stalls > 0 {
            Some(false)
        } else if observed && self.latest != "unknown" {
            Some(true)
        } else {
            None
        };
        json!({"observed":observed,"healthy":healthy,"latest":self.latest,
            "thresholdMs":milliseconds(self.threshold),"minimumRequestedSamples":MIN_REQUESTED_SAMPLES,
            "frames":self.frames,"nativeSamples":self.native_samples,"unknownSamples":self.unknown_samples,
            "requestedSamples":self.requested_samples,"offscreenSamples":self.offscreen_samples,
            "gpuSurfaceSubmissions":self.submissions,"nativePresentations":self.presentations,
            "stalls":self.stalls,"recoveredIntervals":self.recovered,"idleResetIntervals":self.idle_resets,
            "unknownResetIntervals":self.unknown_resets,"activeStall":self.waiting.as_ref().is_some_and(|w| w.stalled),
            "pendingNoOutputMs":self.waiting.as_ref().map(|w| milliseconds(w.elapsed)),
            "longestNoOutputRequestedMs":milliseconds(self.longest_wait),"intervalsOmitted":self.omitted,
            "outputScope":"Aggregate main/floating UI surface submissions, not per-surface freshness or proof of GPU completion",
            "intervals":self.intervals.iter().map(|i| json!({"firstRequestedFrame":i.first_requested_frame,
                "detectedFrame":i.detected_frame,"startedAtMs":milliseconds(i.started),"detectedAtMs":milliseconds(i.detected),
                "noOutputRequestedMs":milliseconds(i.no_output_requested),"offscreenAtDetection":i.offscreen,
                "endedFrame":i.ended_frame,"endedAtMs":i.ended.map(milliseconds),"endReason":i.reason})).collect::<Vec<_>>(),
            "scope":"Native frame observations only. At least two requested samples and ten seconds without a GPU surface submission detect one interval. Idle/unknown resets are not recovery. Presenting an old image is not fresh output. No FPS or frame-spike gate."})
    }
}

fn milliseconds(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
