//! Device-clock normalization and bounded whole-frame timestamp accounting.
use super::*;

struct Calibration {
    get: vk::PFN_vkGetCalibratedTimestampsKHR,
    device: vk::Device,
    origin: u64,
}
impl Calibration {
    fn sample(&self) -> Result<u64> {
        let info = vk::CalibratedTimestampInfoKHR::default().time_domain(vk::TimeDomainKHR::DEVICE);
        let (mut tick, mut deviation) = (0, 0);
        // SAFETY: The enabled extension's function table and logical device stay
        // live in Context. DEVICE is required by both KHR and EXT variants.
        unsafe { (self.get)(self.device, 1, &info, &mut tick, &mut deviation) }
            .result()
            .map_err(anyhow::Error::from)?;
        Ok(tick)
    }
}

pub(super) struct Clock {
    calibrated: Option<Calibration>,
    origins: [Option<(u64, i128)>; 2],
}
impl Clock {
    pub fn new(context: &Context) -> Result<Self> {
        let get = match context.info.timestamp_clock {
            Some("VK_KHR_calibrated_timestamps") => Some(
                ash::khr::calibrated_timestamps::Device::load(&context.instance, &context.device)
                    .fp()
                    .get_calibrated_timestamps_khr,
            ),
            Some("VK_EXT_calibrated_timestamps") => Some(
                ash::ext::calibrated_timestamps::Device::load(&context.instance, &context.device)
                    .fp()
                    .get_calibrated_timestamps_ext,
            ),
            _ => None,
        };
        let calibrated = get
            .map(|get| -> Result<Calibration> {
                let mut value = Calibration {
                    get,
                    device: context.device.handle(),
                    origin: 0,
                };
                value.origin = value.sample()?;
                Ok(value)
            })
            .transpose()?;
        Ok(Self {
            calibrated,
            origins: [None; 2],
        })
    }

    pub fn shared(&self) -> bool {
        self.calibrated.is_some()
    }

    pub fn reference(&mut self, queue: Queue, bits: u32, first: u64) -> Result<Reference> {
        let (tick, elapsed) = if let Some(calibration) = &self.calibrated {
            // Full-width queues need no per-frame driver calibration call. Narrow
            // queries are unwrapped near the completed submission, not near launch.
            let tick = if bits == 64 {
                calibration.origin
            } else {
                calibration.sample()?
            };
            (tick, i128::from(tick.wrapping_sub(calibration.origin)))
        } else {
            let (last, elapsed) = self.origins[queue.index()].get_or_insert((first, 0));
            *elapsed += i128::from(relative_tick(first, *last, bits));
            *last = first;
            (first, *elapsed)
        };
        Ok(Reference {
            tick,
            elapsed,
            bits,
        })
    }
}

pub(super) struct Reference {
    tick: u64,
    elapsed: i128,
    bits: u32,
}
impl Reference {
    pub fn interval(&self, begin: u64, end: u64, period_ns: f32) -> TimestampSpan {
        let mask = u64::MAX >> (64 - self.bits);
        let relative = relative_tick(begin, self.tick, self.bits);
        let scale = f64::from(period_ns) / 1_000_000.;
        // Normalize in integer ticks before converting either endpoint. Adding a
        // floating duration to a floating begin rounds equal end ticks differently
        // for an enclosing submission and its nested passes.
        let start = self.elapsed + i128::from(relative);
        let end = start + i128::from(end.wrapping_sub(begin) & mask);
        TimestampSpan {
            start_ms: start as f64 * scale,
            end_ms: end as f64 * scale,
        }
    }
}

fn relative_tick(tick: u64, origin: u64, bits: u32) -> i64 {
    let shift = 64 - bits;
    (tick.wrapping_sub(origin) << shift) as i64 >> shift
}

#[derive(Default)]
pub(crate) struct Window {
    active: bool,
    after: [u64; 2],
    queues: u8,
    span: Option<TimestampSpan>,
    occupied: Vec<TimestampSpan>,
}
impl Window {
    pub fn reset(&mut self, submitted: [u64; 2]) {
        self.active = true;
        self.after = submitted;
        self.queues = 0;
        self.span = None;
        self.occupied.clear();
    }
    pub fn observe(&mut self, point: TimelinePoint, interval: TimestampSpan) {
        if !self.includes(point) {
            return;
        }
        self.queues |= 1 << point.queue.index();
        self.span = Some(match self.span {
            None => interval,
            Some(old) => TimestampSpan {
                start_ms: old.start_ms.min(interval.start_ms),
                end_ms: old.end_ms.max(interval.end_ms),
            },
        });
        // Keep the union ordered as asynchronous queues resolve. Storage is
        // reused at reset; this scales with submissions, never scene instances.
        let mut interval = interval;
        let first = self
            .occupied
            .partition_point(|old| old.end_ms < interval.start_ms);
        let mut end = first;
        while let Some(old) = self.occupied.get(end)
            && old.start_ms <= interval.end_ms
        {
            interval.start_ms = interval.start_ms.min(old.start_ms);
            interval.end_ms = interval.end_ms.max(old.end_ms);
            end += 1;
        }
        if first == end {
            self.occupied.insert(first, interval);
        } else {
            self.occupied[first] = interval;
            drop(self.occupied.drain(first + 1..end));
        }
    }
    pub fn includes(&self, point: TimelinePoint) -> bool {
        self.active && point.value > self.after[point.queue.index()]
    }
    pub fn span(&self, shared_clock: bool) -> Option<TimestampSpan> {
        if self.queues.count_ones() > 1 && !shared_clock {
            None
        } else {
            self.span
        }
    }
    pub fn gaps(&self, shared_clock: bool) -> Option<f64> {
        self.span(shared_clock)?;
        Some(
            self.occupied
                .windows(2)
                .map(|pair| pair[1].start_ms - pair[0].end_ms)
                .sum(),
        )
    }
}

impl VulkanDevice {
    /// Metal's switch for timestamps outside profiling, where each one costs a driver
    /// dispatch. Vulkan timestamps are query writes inside the command buffer with no
    /// measurable cost, so this backend keeps timing every pass.
    pub fn set_pass_timing(&mut self, _enabled: bool) {}
    /// Start a measurement boundary. Only subsequently submitted, resolved work
    /// contributes; uploads, unlabelled lists and gaps between submissions count.
    pub fn reset_timing_window(&mut self) {
        self.timing_window.reset(self.submitted);
    }
    /// Completed submission span since reset. Mixed queues are unavailable when
    /// the driver cannot guarantee a shared calibrated device clock.
    pub fn timing_window(&self) -> Option<TimestampSpan> {
        if self
            .pending
            .iter()
            .any(|p| self.timing_window.includes(p.point))
        {
            return None;
        }
        self.timing_window.span(self.clock.shared())
    }
    /// Time not covered by any of this frame's submission intervals. Overlapping
    /// queues count once; waits inside a submission remain part of that interval.
    pub fn timing_window_gaps(&self) -> Option<f64> {
        self.timing_window()?;
        self.timing_window.gaps(self.clock.shared())
    }
}

/// Delay a pass's begin until after its leading resource barriers. Empty and
/// barrier-only passes still receive a paired begin/end, with labels preserved.
pub(crate) fn commands(commands: &[Command]) -> impl Iterator<Item = &Command> {
    let mut iter = commands.iter().peekable();
    let mut begin = None;
    std::iter::from_fn(move || {
        if matches!(iter.peek(), Some(Command::BeginPass(_))) {
            begin = iter.next();
        }
        if begin.is_some() && !matches!(iter.peek(), Some(Command::Barrier(_))) {
            return begin.take();
        }
        iter.next()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spans_include_overlap_and_gaps_but_never_mix_unknown_queue_clocks() {
        let mut window = Window::default();
        window.reset([3, 0]);
        for (queue, value, start_ms, end_ms) in [
            (Queue::Graphics, 3, -9., 99.),
            (Queue::Graphics, 4, 1., 4.),
            (Queue::Compute, 1, 2., 3.),
            (Queue::Graphics, 5, 6., 7.),
        ] {
            window.observe(
                TimelinePoint { queue, value },
                TimestampSpan { start_ms, end_ms },
            );
        }
        assert_eq!(window.span(true).map(TimestampSpan::milliseconds), Some(6.));
        assert_eq!(window.gaps(true), Some(2.));
        assert_eq!(window.span(false), None);
        window.reset([5, 1]);
        assert_eq!(window.span(true), None);
    }
    #[test]
    fn submission_union_is_order_independent_and_storage_is_inactive_until_reset() {
        let mut window = Window::default();
        let point = TimelinePoint {
            queue: Queue::Graphics,
            value: 1,
        };
        window.observe(
            point,
            TimestampSpan {
                start_ms: 1.,
                end_ms: 99.,
            },
        );
        assert!(window.occupied.is_empty());
        assert!(window.span(true).is_none());
        window.reset([0; 2]);
        for (value, start_ms, end_ms) in [(1, 3., 5.), (2, 1., 4.), (3, 6., 8.), (4, 4.5, 6.5)] {
            window.observe(
                TimelinePoint { value, ..point },
                TimestampSpan { start_ms, end_ms },
            );
        }
        assert_eq!(window.gaps(true), Some(0.));
        assert_eq!(
            window.span(true),
            Some(TimestampSpan {
                start_ms: 1.,
                end_ms: 8.
            })
        );
        window.reset([4, 0]);
        window.observe(
            TimelinePoint { value: 5, ..point },
            TimestampSpan {
                start_ms: 10.,
                end_ms: 12.,
            },
        );
        assert_eq!(window.gaps(false), Some(0.));
    }
    #[test]
    fn narrow_timestamp_wrap_preserves_duration_and_common_epoch() {
        let reference = Reference {
            tick: 260,
            elapsed: 20,
            bits: 8,
        };
        let span = reference.interval(254, 2, 1_000_000.);
        assert_eq!(
            span,
            TimestampSpan {
                start_ms: 14.,
                end_ms: 18.
            }
        );
        let mut uncalibrated = Clock {
            calibrated: None,
            origins: [None; 2],
        };
        let first = uncalibrated
            .reference(Queue::Graphics, 8, 250)
            .unwrap()
            .interval(250, 254, 1_000_000.);
        let second = uncalibrated
            .reference(Queue::Graphics, 8, 2)
            .unwrap()
            .interval(2, 4, 1_000_000.);
        assert_eq!(
            first,
            TimestampSpan {
                start_ms: 0.,
                end_ms: 4.
            }
        );
        assert_eq!(
            second,
            TimestampSpan {
                start_ms: 8.,
                end_ms: 10.
            }
        );
    }
    #[test]
    fn equal_raw_endpoints_are_bit_identical_across_nested_intervals() {
        let reference = Reference {
            tick: 0,
            elapsed: 0,
            bits: 64,
        };
        let endpoint = reference.interval(12_340, 12_345, 1.).end_ms;
        for begin in 0..1_000 {
            assert_eq!(reference.interval(begin, 12_345, 1.).end_ms, endpoint);
        }
    }
    #[test]
    fn leading_barriers_precede_begin_but_trailing_barriers_remain_inside() {
        // A pure ordering test uses real barriers from an arena handle.
        let mut arena = crate::arena::Arena::<crate::BufferKind, ()>::default();
        let buffer = arena.insert(()).unwrap();
        let barrier = || {
            Command::Barrier(Barrier {
                resource: Resource::Buffer(buffer),
                before: Access::HostWrite,
                after: Access::Storage,
                handoff: None,
            })
        };
        let list = [
            Command::BeginPass("one".into()),
            barrier(),
            barrier(),
            Command::Dispatch([1; 3]),
            barrier(),
            Command::EndPass,
            Command::BeginPass("empty".into()),
            Command::EndPass,
            Command::BeginPass("barriers".into()),
            barrier(),
            Command::EndPass,
        ];
        let labels = commands(&list)
            .map(|c| match c {
                Command::BeginPass(s) => s.as_str(),
                Command::EndPass => "end",
                Command::Barrier(_) => "barrier",
                _ => "work",
            })
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "barrier", "barrier", "one", "work", "barrier", "end", "empty", "end", "barrier",
                "barriers", "end"
            ]
        );
    }
}
