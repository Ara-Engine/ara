//! Fail-closed pass contracts shared by player windows and offline capture timings.
use serde_json::{Value, json};
use std::{fmt, time::Duration};

/// A timing window did not execute its complete expected GPU workload.
pub const INVALID_PASS_SET: ara_diag::Code = ara_diag::Code(6093);
const HYBRID: &[&str] = &[
    "RT.Reflect",
    "RT.Filter.X",
    "RT.Filter.Y",
    "RT.Rough",
    "RT.Rough.X",
    "RT.Rough.Y",
];
const TRACE: &[&str] = &["RT.Reflections.SoftShadows.DDGI"];
const RESERVOIRS: &[&str] = &[
    "ReSTIR.Candidates.Temporal",
    "ReSTIR.GI.CompactTrace",
    "ReSTIR.Specular.Dense",
    "ReSTIR.Shade",
];

/// The workload selected by the preset and device, independently of observed passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    /// A raster preset without ray-traced reflections.
    Raster,
    /// High/Epic raster plus the complete D29 ray-query reflection chain.
    Hybrid,
    /// The explicitly selected fidelity transport, with or without reservoirs.
    Fidelity,
}
impl Expected {
    /// Resolve from requested options and device capabilities, never from timing output.
    pub fn resolve(preset: &str, hardware_rt: bool, fidelity: bool) -> Self {
        if fidelity && preset != "low" {
            Self::Fidelity
        } else if hardware_rt && matches!(preset, "high" | "epic") {
            Self::Hybrid
        } else {
            Self::Raster
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Raster => "raster",
            Self::Hybrid => "hybrid",
            Self::Fidelity => "fidelity",
        }
    }
}

/// Allocation-free presence snapshot of one completed frame's timestamp pass names.
#[derive(Clone, Copy, Default)]
pub struct Frame {
    bits: u16,
    nonempty: bool,
}
impl Frame {
    /// Names may repeat; durations and unrelated RT passes cannot satisfy a contract.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut result = Self::default();
        for name in names {
            result.nonempty = true;
            for (i, required) in HYBRID.iter().chain(TRACE).chain(RESERVOIRS).enumerate() {
                if name == *required {
                    result.bits |= 1 << i;
                }
            }
        }
        result
    }
    fn contains(self, set: Set) -> bool {
        let mask = match set {
            Set::Raster => 0,
            Set::Hybrid => 0x3f,
            Set::Trace => 1 << 6,
            Set::Reservoirs => 0x780,
        };
        self.nonempty && self.bits & mask == mask
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    Raster,
    Hybrid,
    Trace,
    Reservoirs,
}
impl Set {
    fn names(self) -> &'static [&'static str] {
        match self {
            Self::Raster => &[],
            Self::Hybrid => HYBRID,
            Self::Trace => TRACE,
            Self::Reservoirs => RESERVOIRS,
        }
    }
}

/// A coded refusal plus the incomplete window's evidence; never a valid timing.
#[derive(Debug)]
pub struct PassError {
    detail: String,
    evidence: Value,
}
impl PassError {
    /// Structured evidence suitable for a failed timing JSON file.
    pub fn evidence(&self) -> &Value {
        &self.evidence
    }
}
impl fmt::Display for PassError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{INVALID_PASS_SET}: {}", self.detail)
    }
}
impl std::error::Error for PassError {}

/// Require consecutive complete frames before a window, then reject any missing pass.
pub struct Guard {
    expected: Expected,
    stable_required: u32,
    timeout: Duration,
    streak: u32,
    set: Option<Set>,
    timed: u64,
    present: u64,
    failure: Option<String>,
}
impl Guard {
    /// Defaults to three complete frames and the caller's bounded readiness deadline.
    /// ARA_TIMING_STABLE_FRAMES (1..120), ARA_TIMING_READY_TIMEOUT_MS (1..600000),
    /// and ARA_TIMING_EXPECTED_RT (auto/raster/hybrid/fidelity) are recorded overrides.
    pub fn configured(expected: Expected, timeout: Duration) -> Result<Self, PassError> {
        let invalid = |detail: String| PassError {
            detail,
            evidence: Value::Null,
        };
        let expected = match std::env::var("ARA_TIMING_EXPECTED_RT") {
            Err(std::env::VarError::NotPresent) => expected,
            Ok(value) => match value.as_str() {
                "auto" => expected,
                "raster" => Expected::Raster,
                "hybrid" => Expected::Hybrid,
                "fidelity" => Expected::Fidelity,
                _ => return Err(invalid(format!("invalid ARA_TIMING_EXPECTED_RT={value}"))),
            },
            Err(_) => return Err(invalid("invalid ARA_TIMING_EXPECTED_RT".into())),
        };
        let number = |key: &str, default, max| -> Result<u64, PassError> {
            let value = match std::env::var(key) {
                Ok(v) => v
                    .parse::<u64>()
                    .map_err(|_| invalid(format!("invalid {key}")))?,
                Err(std::env::VarError::NotPresent) => default,
                Err(_) => return Err(invalid(format!("invalid {key}"))),
            };
            if value == 0 || value > max {
                return Err(invalid(format!("{key} must be 1..{max}")));
            }
            Ok(value)
        };
        let stable = number("ARA_TIMING_STABLE_FRAMES", 3, 120)? as u32;
        let timeout = Duration::from_millis(number(
            "ARA_TIMING_READY_TIMEOUT_MS",
            timeout.as_millis().min(600_000) as u64,
            600_000,
        )?);
        Ok(Self::new(expected, stable, timeout))
    }
    /// Construct a deterministic guard; callers must use a positive count and deadline.
    pub fn new(expected: Expected, stable_frames: u32, timeout: Duration) -> Self {
        Self {
            expected,
            stable_required: stable_frames.max(1),
            timeout,
            streak: 0,
            set: None,
            timed: 0,
            present: 0,
            failure: None,
        }
    }
    fn candidate(&self, frame: Frame) -> Option<Set> {
        [Set::Raster, Set::Hybrid, Set::Trace, Set::Reservoirs]
            .into_iter()
            .find(|&set| {
                matches!(
                    (self.expected, set),
                    (Expected::Raster, Set::Raster)
                        | (Expected::Hybrid, Set::Hybrid)
                        | (Expected::Fidelity, Set::Trace | Set::Reservoirs)
                ) && frame.contains(set)
            })
    }
    /// No completed frame (`None`) neither advances nor resets consecutive GPU frames.
    /// Keep calling through ordinary warmup, so late workload loss resets stability.
    pub fn wait(&mut self, elapsed: Duration, frame: Option<Frame>) -> Result<bool, PassError> {
        if self.failure.is_some() {
            return self.fail("pass contract already failed".into());
        }
        if elapsed >= self.timeout {
            return self.fail(format!(
                "{} passes did not stabilize before {} ms",
                self.expected.name(),
                self.timeout.as_millis()
            ));
        }
        let Some(frame) = frame else {
            return Ok(false);
        };
        match self.candidate(frame) {
            Some(set) => {
                self.streak = if self.set == Some(set) {
                    self.streak.saturating_add(1)
                } else {
                    1
                };
                self.set = Some(set);
            }
            None => {
                self.streak = 0;
                self.set = None;
            }
        }
        Ok(self.set.is_some()
            && (self.expected == Expected::Raster || self.streak >= self.stable_required))
    }
    /// Validate before storing the frame's durations. A missing pass permanently fails it.
    pub fn record(&mut self, frame: Frame) -> Result<(), PassError> {
        self.timed += 1;
        let ready = self.expected == Expected::Raster || self.streak >= self.stable_required;
        let complete = ready && self.set.is_some_and(|set| frame.contains(set));
        if self.failure.is_some() || !complete {
            return self.fail(format!(
                "timed frame {} lost expected {} passes {:?}; rt present: {}/{} frames",
                self.timed,
                self.expected.name(),
                self.required(),
                self.present,
                self.timed
            ));
        }
        if self.expected != Expected::Raster {
            self.present += 1;
        }
        Ok(())
    }
    fn required(&self) -> &'static [&'static str] {
        self.set.map(Set::names).unwrap_or(match self.expected {
            Expected::Raster => &[],
            Expected::Hybrid => HYBRID,
            Expected::Fidelity => TRACE,
        })
    }
    fn fail<T>(&mut self, detail: String) -> Result<T, PassError> {
        self.failure = Some(detail.clone());
        Err(PassError {
            detail,
            evidence: self.report(),
        })
    }
    /// Timing JSON counts complete expected RT workloads, including a rejected frame.
    pub fn report(&self) -> Value {
        json!({"format":"ara.timing.passes/1","expected":self.expected.name(),"required_passes":self.required(),"stable_frames_required":self.stable_required,"stable_frames_observed":self.streak,"timeout_ms":self.timeout.as_millis(),"timed_frames":self.timed,"rt_present_frames":self.present,"rt_present":format!("{}/{} frames",self.present,self.timed),"status":if self.failure.is_some(){"invalid"}else if self.timed>0{"valid"}else{"waiting"},"diagnostic":self.failure.as_ref().map(|_|INVALID_PASS_SET.to_string()),"failure":self.failure,"definition":"RT present means the complete expected reflection workload, not just any RT-prefixed pass"})
    }
}

/// Refuse old, partial or failed evidence before exposing a number to perf/D26/A-B.
pub fn validate(evidence: &Value, frames: u64) -> Result<(), PassError> {
    let expected = evidence["expected"].as_str();
    let rt = matches!(expected, Some("hybrid" | "fidelity"));
    let required = &evidence["required_passes"];
    let required_valid = match expected {
        Some("raster") => required == &json!([]),
        Some("hybrid") => required == &json!(HYBRID),
        Some("fidelity") => required == &json!(TRACE) || required == &json!(RESERVOIRS),
        _ => false,
    };
    let valid = frames > 0
        && evidence["format"] == "ara.timing.passes/1"
        && required_valid
        && matches!(expected, Some("raster" | "hybrid" | "fidelity"))
        && evidence["status"] == "valid"
        && evidence["timed_frames"].as_u64() == Some(frames)
        && (!rt
            || (evidence["rt_present_frames"].as_u64() == Some(frames)
                && evidence["stable_frames_required"]
                    .as_u64()
                    .is_some_and(|n| {
                        n > 0
                            && evidence["stable_frames_observed"]
                                .as_u64()
                                .is_some_and(|seen| seen >= n)
                    })));
    if valid {
        Ok(())
    } else {
        Err(PassError {
            detail: "missing or incomplete per-frame pass-presence evidence".into(),
            evidence: evidence.clone(),
        })
    }
}

#[cfg(test)]
mod tests;
