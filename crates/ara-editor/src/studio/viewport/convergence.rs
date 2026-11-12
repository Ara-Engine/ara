//! Completed primary frames, not UI polls or merely submitted frames.
/// Most frames an input may add while automatic exposure adapts after the jitter
/// cycles ([`Convergence::adapt`]): two seconds at 60 Hz, which lands the default rate
/// (2/s) within 1%; a meter that keeps moving (flicker in view) stops there.
const ADAPT_FRAMES: u32 = 120;
#[derive(Debug)]
pub(super) struct Convergence {
    budget: u32,
    remaining: u32,
    submitted: u32,
    /// Frames [`Convergence::adapt`] has added since the last input.
    adapted: u32,
}
impl Default for Convergence {
    fn default() -> Self {
        Self {
            budget: 24,
            remaining: 0,
            submitted: 0,
            adapted: 0,
        }
    }
}
impl Convergence {
    pub fn configure(&mut self, phases: u32) {
        let budget = 3 * phases;
        if self.budget != budget {
            self.budget = budget;
            self.reset();
        }
    }
    /// Repeated input extends one budget; it never stacks budgets. A pending
    /// older image must not consume samples required by the new input.
    pub fn reset(&mut self) {
        self.adapted = 0;
        self.remaining = self.remaining.max(self.budget);
        self.submitted = 0;
    }
    /// One more frame without restarting convergence (an image-only change).
    pub fn redraw(&mut self) {
        self.remaining = self.remaining.max(1);
    }
    /// One more frame while automatic exposure still adapts after the budget, at
    /// most [`ADAPT_FRAMES`] per input.
    pub fn adapt(&mut self) {
        if self.adapted < ADAPT_FRAMES {
            self.adapted += 1;
            self.remaining = self.remaining.max(1);
        }
    }
    pub fn needs_frame(&self) -> bool {
        self.remaining > 0
    }
    pub fn submitted(&mut self, frames: u32) {
        self.submitted = frames;
    }
    pub fn completed(&mut self) {
        self.remaining = self.remaining.saturating_sub(self.submitted);
        self.submitted = 0;
    }
    pub fn failed(&mut self) {
        self.remaining = 0;
        self.submitted = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budgets_follow_the_renderers_actual_halton_sequence() {
        let mut budget = Convergence::default();
        budget.configure(ara_render::temporal_phases([1325, 859], [1987, 1288]));
        assert_eq!(budget.budget, 54);
        budget.configure(ara_render::temporal_phases([2560, 1440], [2560, 1440]));
        assert_eq!(budget.budget, 24);
    }
    #[test]
    fn three_phase_cycles_are_consumed_only_once_per_completed_frame() {
        let mut budget = Convergence::default();
        budget.configure(18);
        for _ in 0..20 {
            budget.completed();
        }
        assert_eq!(budget.remaining, 54, "polling submits or consumes nothing");
        for _ in 0..53 {
            budget.submitted(1);
            budget.completed();
            assert!(budget.needs_frame());
        }
        budget.submitted(1);
        assert!(budget.needs_frame());
        budget.completed();
        assert!(!budget.needs_frame());
    }
    #[test]
    fn exposure_adaptation_extends_a_still_view_boundedly_until_the_next_input() {
        let mut budget = Convergence::default();
        budget.configure(8);
        budget.reset();
        for _ in 0..24 {
            budget.submitted(1);
            budget.completed();
        }
        assert!(!budget.needs_frame());
        let mut extra = 0;
        for _ in 0..1000 {
            budget.adapt();
            if !budget.needs_frame() {
                break;
            }
            budget.submitted(1);
            budget.completed();
            extra += 1;
        }
        assert_eq!(extra, ADAPT_FRAMES, "a meter that never settles stops");
        budget.reset();
        budget.remaining = 0;
        budget.adapt();
        assert!(
            budget.needs_frame(),
            "new input grants adaptation frames again"
        );
    }
    #[test]
    fn repeated_resets_do_not_stack_and_obsolete_frames_do_not_consume_new_input() {
        let mut budget = Convergence::default();
        budget.reset();
        budget.submitted(1);
        budget.reset();
        budget.completed();
        assert_eq!(budget.remaining, 24);
        for _ in 0..20 {
            budget.reset();
        }
        assert_eq!(budget.remaining, 24);
        budget.failed();
        assert!(!budget.needs_frame());
        budget.configure(32);
        assert_eq!(budget.remaining, 96);
    }
}
