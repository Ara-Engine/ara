//! `ARA_STUDIO_TRACE=1`: what the studio spends its time on, on stderr. Every input event,
//! every loop pass slower than a frame (by phase), and every editor action or content commit
//! slower than 5 ms. Off, it costs one cached flag read.
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Whether tracing is on (read once).
pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ARA_STUDIO_TRACE").is_some())
}

/// Logs `what` when tracing, with the time since the studio started.
pub(super) fn log(what: impl FnOnce() -> String) {
    if on() {
        static START: OnceLock<Instant> = OnceLock::new();
        let t = START.get_or_init(Instant::now).elapsed();
        eprintln!("[studio {:>9.3}s] {}", t.as_secs_f64(), what());
    }
}

/// Runs `f`, logging `what` with its duration when it took longer than `slow`.
pub(super) fn time<R>(what: &str, slow: Duration, f: impl FnOnce() -> R) -> R {
    if !on() {
        return f();
    }
    let at = Instant::now();
    let out = f();
    let took = at.elapsed();
    if took >= slow {
        log(|| format!("{what} took {took:.2?}"));
    }
    out
}

/// Phase timings of one loop pass, logged when the pass was slower than a frame.
#[derive(Default)]
pub(super) struct Pass {
    start: Option<Instant>,
    last: Option<Instant>,
    phases: Vec<(&'static str, Duration)>,
}

impl Pass {
    /// Starts timing a pass.
    pub fn begin() -> Self {
        let now = on().then(Instant::now);
        Self {
            start: now,
            last: now,
            phases: Vec::new(),
        }
    }

    /// Ends phase `name` here.
    pub fn mark(&mut self, name: &'static str) {
        if let Some(last) = self.last {
            let now = Instant::now();
            self.phases.push((name, now - last));
            self.last = Some(now);
        }
    }

    /// Logs the pass if it took longer than 16 ms.
    pub fn end(self) {
        if let Some(start) = self.start {
            let total = start.elapsed();
            if total >= Duration::from_millis(16) {
                let parts: Vec<String> = self
                    .phases
                    .iter()
                    .filter(|(_, d)| *d >= Duration::from_millis(1))
                    .map(|(n, d)| format!("{n} {d:.1?}"))
                    .collect();
                log(|| format!("slow pass {total:.1?}: {}", parts.join(", ")));
            }
        }
    }
}
