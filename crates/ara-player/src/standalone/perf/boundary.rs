//! Steady GPU samples begin after readiness, then a complete warmup interval.
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Step {
    Waiting,
    Warmup,
    Sample,
    TimedOut,
}

pub(super) struct Boundary {
    warmup: u32,
    timeout: Duration,
    ready_frame: Option<u32>,
    ready_ms: Option<f64>,
    first_sample: Option<u32>,
}
impl Boundary {
    pub fn new(warmup: u32, timeout: Duration) -> Self {
        Self {
            warmup,
            timeout,
            ready_frame: None,
            ready_ms: None,
            first_sample: None,
        }
    }
    pub fn observe(&mut self, frame: u32, elapsed: Duration, ready: bool) -> Step {
        if self.ready_frame.is_none() && elapsed >= self.timeout {
            return Step::TimedOut;
        }
        if !ready && self.ready_frame.is_none() {
            return Step::Waiting;
        }
        let Some(ready_frame) = self.ready_frame else {
            self.ready_frame = Some(frame);
            self.ready_ms = Some(elapsed.as_secs_f64() * 1000.);
            return Step::Warmup;
        };
        if frame.saturating_sub(ready_frame) <= self.warmup {
            return Step::Warmup;
        }
        self.first_sample.get_or_insert(frame);
        Step::Sample
    }
    pub fn confirmed(&self) -> bool {
        self.ready_frame.is_some()
    }
    pub fn report(&self) -> Value {
        json!({"ready_frame":self.ready_frame,"time_to_ready_ms":self.ready_ms,
            "gpu_start_frame":self.first_sample,"warmup_frames":self.warmup,
            "timeout_ms":self.timeout.as_secs_f64()*1000.})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_precedes_the_whole_configured_warmup() {
        let mut boundary = Boundary::new(60, Duration::from_secs(120));
        for frame in 1..430 {
            assert_eq!(
                boundary.observe(frame, Duration::from_millis(frame.into()), false),
                Step::Waiting
            );
        }
        for frame in 430..=490 {
            assert_eq!(
                boundary.observe(frame, Duration::from_millis(frame.into()), true),
                Step::Warmup
            );
        }
        assert_eq!(
            boundary.observe(491, Duration::from_millis(491), true),
            Step::Sample
        );
        assert_eq!(boundary.report()["ready_frame"], 430);
        assert_eq!(boundary.report()["gpu_start_frame"], 491);
    }
    #[test]
    fn missing_or_late_readiness_times_out_even_if_it_arrives_at_the_deadline() {
        for ready in [false, true] {
            let mut boundary = Boundary::new(0, Duration::from_secs(2));
            assert_eq!(
                boundary.observe(1, Duration::from_secs(1), false),
                Step::Waiting
            );
            assert_eq!(
                boundary.observe(2, Duration::from_secs(2), ready),
                Step::TimedOut
            );
            assert!(boundary.report()["gpu_start_frame"].is_null());
        }
    }
    #[test]
    fn a_confirmed_signal_allows_ordinary_in_flight_frames_during_warmup() {
        let mut boundary = Boundary::new(2, Duration::from_secs(2));
        assert_eq!(boundary.observe(1, Duration::ZERO, true), Step::Warmup);
        assert_eq!(boundary.observe(2, Duration::ZERO, false), Step::Warmup);
        assert_eq!(
            boundary.observe(3, Duration::from_secs(1), false),
            Step::Warmup
        );
        assert_eq!(
            boundary.observe(4, Duration::from_secs(2), false),
            Step::Sample
        );
        assert!(boundary.confirmed());
    }
}
