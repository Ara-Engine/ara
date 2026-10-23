//! The diagnostics bus: any thread emits [`Diagnostic`]s; one consumer per process
//! (the project host) drains them into a [`Store`](crate::store::Store).
//!
//! ```ignore
//! // Anywhere, any thread. Lock-free; a no-op until a consumer attaches.
//! Diagnostic::error(codes::ASSET_IMPORT_FAILED, "`crate.glb` has no meshes")
//!     .subject(Subject::asset("@meshes/crate.glb"))
//!     .emit();
//! // Hot paths: check admission before formatting anything.
//! ara_diag::emit!(warning, codes::FRAME_OVER_BUDGET, "frame {frame} took {ms:.1} ms");
//! ```
//!
//! - **Per-thread rings.** Each emitting thread gets a bounded SPSC ring on its
//!   first emit; pushing never locks or contends. A full ring drops the event and
//!   counts it (reported as `ARA2001`).
//! - **Sampling.** [`admit`] lets the first [`DEFAULT_BURST`] events of each code
//!   through per thread per drain window and only counts the rest; the store adds
//!   the counts to the code's latest record. [`set_burst`] tunes spammy codes.
//! - **Off by default.** Until a [`Consumer`] is attached, [`admit`] is one relaxed
//!   load that says no, so libraries pay nothing in processes without a bus.
//! - **Metadata.** Each event carries an [`EventId`] (for causal links), a
//!   timestamp, the current simulation tick ([`set_tick`]) and an [`Origin`]
//!   ([`scope`] marks work done for a human or an agent).

mod ring;

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Code, Diagnostic};
use ring::Ring;

/// Events each thread's ring holds before new ones are dropped.
pub const RING_CAPACITY: usize = 512;
/// Events of one code admitted per thread per drain window before sampling starts.
pub const DEFAULT_BURST: u32 = 32;
/// Distinct codes a thread tracks per window; codes past this are never sampled.
const TRACKED_CODES: usize = 32;
/// Per-code burst overrides ([`set_burst`]).
const OVERRIDES: usize = 64;

/// Identifies one emitted event, for causal links (`Meta::cause`). Unique per
/// process: the emitting thread's slot in the high bits, its counter in the low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub u64);

/// Who caused a diagnostic: the engine itself, or work done for a human or an agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Origin {
    /// Engine work nobody asked for directly (ticks, imports, streaming).
    #[default]
    Engine,
    /// A human's action (an editor edit, a CLI command).
    Human,
    /// An agent's action; the name is the author's (`agent:<name>`).
    Agent(Arc<str>),
}

impl Origin {
    /// The origin of an author string (`human`, `agent:claude`); anything else is
    /// the engine.
    pub fn of_author(author: &str) -> Origin {
        match author.strip_prefix("agent:") {
            Some(name) => Origin::Agent(name.into()),
            None if author == "human" => Origin::Human,
            None => Origin::Engine,
        }
    }

    /// The kind without the agent name (`engine`, `human`, `agent`).
    pub fn kind(&self) -> &'static str {
        match self {
            Origin::Engine => "engine",
            Origin::Human => "human",
            Origin::Agent(_) => "agent",
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Agent(name) => write!(f, "agent:{name}"),
            other => f.write_str(other.kind()),
        }
    }
}

impl Serialize for Origin {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Origin {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Ok(match text.as_str() {
            "engine" => Origin::Engine,
            other => Origin::of_author(other),
        })
    }
}

/// Optional metadata for [`push`] and [`emit_with`]. Unset parts default to the
/// thread's [`scope`] origin, the bus tick and a content-based dedup key.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    /// The event that caused this one.
    pub cause: Option<EventId>,
    /// Explicit de-duplication key, for diagnostics whose message varies between
    /// occurrences of the same problem (frame times, counts).
    pub dedup: Option<u64>,
    /// Who caused it.
    pub origin: Option<Origin>,
    /// Simulation tick.
    pub tick: Option<u64>,
}

impl Meta {
    /// Metadata linking to the causing event.
    pub fn caused_by(cause: Option<EventId>) -> Meta {
        Meta {
            cause,
            ..Meta::default()
        }
    }

    /// Sets the dedup key from any text (`"asset:@meshes/crate.glb"`).
    pub fn dedup(mut self, key: &str) -> Meta {
        self.dedup = Some(fnv(key.as_bytes()));
        self
    }
}

/// One emitted diagnostic with its metadata, as drained from the rings.
#[derive(Debug, Clone)]
pub struct Event {
    /// Its ID.
    pub id: EventId,
    /// The event that caused it.
    pub cause: Option<EventId>,
    /// Explicit dedup key.
    pub dedup: Option<u64>,
    /// Who caused it.
    pub origin: Origin,
    /// Simulation tick when it was emitted.
    pub tick: Option<u64>,
    /// When it was emitted.
    pub at: Instant,
    /// The diagnostic.
    pub diagnostic: Diagnostic,
}

/// FNV-1a, for dedup keys (stable across runs, unlike `DefaultHasher`).
pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    let mut h = Fnv::default();
    std::hash::Hasher::write(&mut h, bytes);
    h.0
}

/// Streaming FNV-1a: a `Hasher` for the store's maps and a `fmt::Write` sink, so
/// dedup keys hash formatted parts without allocating.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Fnv(pub(crate) u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl std::hash::Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn write_u64(&mut self, n: u64) {
        // Keys are already hashes or sequence numbers: one multiply mixes enough.
        self.0 = (self.0 ^ n).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

impl fmt::Write for Fnv {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        std::hash::Hasher::write(self, s.as_bytes());
        Ok(())
    }
}

/// A thread's ring plus the counters the consumer reads.
struct Shared {
    ring: Ring<Event>,
    /// Events lost to a full ring since the last drain.
    dropped: AtomicU64,
    /// Sampled-out events per code since the last drain: `(code + 1, count)`.
    sampled: [(AtomicU32, AtomicU32); TRACKED_CODES],
}

impl Shared {
    fn new() -> Self {
        Shared {
            ring: Ring::new(RING_CAPACITY),
            dropped: AtomicU64::new(0),
            sampled: std::array::from_fn(|_| (AtomicU32::new(0), AtomicU32::new(0))),
        }
    }

    /// Counts one sampled-out event of `code` (producer side).
    fn sample(&self, code: Code) {
        let key = u32::from(code.0) + 1;
        for (slot, count) in &self.sampled {
            let held = slot.load(Ordering::Relaxed);
            if held == key
                || (held == 0
                    && slot
                        .compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok())
            {
                count.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }
}

struct Global {
    consumers: AtomicUsize,
    enabled: AtomicBool,
    /// Current simulation tick plus one; 0 = none.
    tick: AtomicU64,
    /// Drain generation; sampling windows restart when it changes.
    window: AtomicU64,
    next_slot: AtomicU32,
    /// `(code + 1, burst)` overrides.
    bursts: [(AtomicU32, AtomicU32); OVERRIDES],
    rings: Mutex<Vec<Arc<Shared>>>,
}

static GLOBAL: Global = Global {
    consumers: AtomicUsize::new(0),
    enabled: AtomicBool::new(false),
    tick: AtomicU64::new(0),
    window: AtomicU64::new(0),
    next_slot: AtomicU32::new(1),
    bursts: [const { (AtomicU32::new(0), AtomicU32::new(0)) }; OVERRIDES],
    rings: Mutex::new(Vec::new()),
};

fn burst(code: Code) -> u32 {
    let key = u32::from(code.0) + 1;
    GLOBAL
        .bursts
        .iter()
        .take_while(|(k, _)| k.load(Ordering::Relaxed) != 0)
        .find(|(k, _)| k.load(Ordering::Relaxed) == key)
        .map_or(DEFAULT_BURST, |(_, b)| b.load(Ordering::Relaxed))
}

/// The emitting thread's producer state.
struct Local {
    shared: Option<Arc<Shared>>,
    slot: u32,
    next: u32,
    window: u64,
    /// `(code + 1, admitted this window)`.
    counts: [(u32, u32); TRACKED_CODES],
}

impl Local {
    /// The thread's ring, registered with the consumer on first use.
    fn shared(&mut self) -> &Shared {
        if self.shared.is_none() {
            self.slot = GLOBAL.next_slot.fetch_add(1, Ordering::Relaxed);
            let shared = Arc::new(Shared::new());
            GLOBAL
                .rings
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Arc::clone(&shared));
            self.shared = Some(shared);
        }
        self.shared.as_deref().expect("the ring was just registered")
    }

    /// The next event ID of this thread.
    fn next_id(&mut self) -> EventId {
        let id = EventId((u64::from(self.slot) << 32) | u64::from(self.next));
        self.next = self.next.wrapping_add(1);
        id
    }

    fn admit(&mut self, code: Code) -> bool {
        let window = GLOBAL.window.load(Ordering::Relaxed);
        if window != self.window {
            self.window = window;
            self.counts = [(0, 0); TRACKED_CODES];
        }
        let key = u32::from(code.0) + 1;
        let Some(entry) = self
            .counts
            .iter_mut()
            .find(|(k, _)| *k == key || *k == 0)
        else {
            return true;
        };
        entry.0 = key;
        entry.1 = entry.1.saturating_add(1);
        if entry.1 <= burst(code) {
            return true;
        }
        self.shared().sample(code);
        false
    }
}

thread_local! {
    static LOCAL: RefCell<Local> = const {
        RefCell::new(Local {
            shared: None,
            slot: 0,
            next: 0,
            window: 0,
            counts: [(0, 0); TRACKED_CODES],
        })
    };
    static ORIGIN: RefCell<Option<Origin>> = const { RefCell::new(None) };
}

/// True if a diagnostic of `code` should be emitted now: a consumer is attached and
/// the code is within its per-thread burst. Check it before building a message on
/// a hot path; [`Diagnostic::emit`] checks it too.
#[inline]
pub fn admit(code: Code) -> bool {
    if !GLOBAL.enabled.load(Ordering::Relaxed) {
        return false;
    }
    LOCAL
        .try_with(|l| l.try_borrow_mut().is_ok_and(|mut l| l.admit(code)))
        .unwrap_or(false)
}

/// Emits an admitted diagnostic without checking [`admit`] again. Returns its ID, or
/// `None` when the thread's ring is full (counted) or the thread is shutting down.
pub fn push(diagnostic: Diagnostic, meta: Meta) -> Option<EventId> {
    let origin = meta
        .origin
        .or_else(|| ORIGIN.try_with(|o| o.borrow().clone()).ok().flatten())
        .unwrap_or_default();
    let tick = meta.tick.or_else(tick);
    LOCAL
        .try_with(|l| {
            let mut l = l.try_borrow_mut().ok()?;
            l.shared();
            let id = l.next_id();
            let shared = l.shared();
            let event = Event {
                id,
                cause: meta.cause,
                dedup: meta.dedup,
                origin,
                tick,
                at: Instant::now(),
                diagnostic,
            };
            match shared.ring.push(event) {
                Ok(()) => Some(id),
                Err(_) => {
                    shared.dropped.fetch_add(1, Ordering::Relaxed);
                    None
                }
            }
        })
        .ok()
        .flatten()
}

/// Emits `diagnostic` with `meta` if [`admit`] allows it.
pub fn emit_with(diagnostic: Diagnostic, meta: Meta) -> Option<EventId> {
    if admit(diagnostic.code) {
        push(diagnostic, meta)
    } else {
        None
    }
}

/// Sets the simulation tick stamped on later events (all threads).
pub fn set_tick(tick: u64) {
    GLOBAL.tick.store(tick.saturating_add(1), Ordering::Relaxed);
}

/// The current simulation tick, if one was set.
pub fn tick() -> Option<u64> {
    GLOBAL.tick.load(Ordering::Relaxed).checked_sub(1)
}

/// Admits `burst` events of `code` per thread per drain window (default
/// [`DEFAULT_BURST`]). Use a small burst for codes that can fire every frame.
pub fn set_burst(code: Code, burst: u32) {
    let key = u32::from(code.0) + 1;
    for (k, b) in &GLOBAL.bursts {
        let held = k.load(Ordering::Relaxed);
        if held == key
            || (held == 0
                && k.compare_exchange(0, key, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok())
        {
            b.store(burst, Ordering::Relaxed);
            return;
        }
    }
}

/// While alive, events emitted on this thread without an explicit origin are
/// attributed to `origin`. Restores the previous origin when dropped.
#[must_use = "the origin applies only while the guard lives"]
pub struct OriginScope {
    previous: Option<Origin>,
}

/// Attributes this thread's events to `origin` until the guard drops.
pub fn scope(origin: Origin) -> OriginScope {
    let previous = ORIGIN
        .try_with(|o| o.borrow_mut().replace(origin))
        .ok()
        .flatten();
    OriginScope { previous }
}

impl Drop for OriginScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = ORIGIN.try_with(|o| *o.borrow_mut() = previous);
    }
}

/// What one [`Consumer::drain`] collected besides the events.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Lost {
    /// Events dropped because a thread's ring was full.
    pub dropped: u64,
    /// Events sampled out, per code.
    pub sampled: Vec<(Code, u32)>,
}

/// The process's bus consumer. While at least one is alive the bus is enabled.
/// Drains are serialized; one consumer per process is the intended setup (the
/// project host), since each event goes to whichever consumer drains it first.
pub struct Consumer {
    _private: (),
}

/// Attaches a consumer, enabling the bus.
pub fn attach() -> Consumer {
    GLOBAL.consumers.fetch_add(1, Ordering::SeqCst);
    GLOBAL.enabled.store(true, Ordering::SeqCst);
    Consumer { _private: () }
}

impl Consumer {
    /// Moves every waiting event into `out` (oldest first per thread; sort by `at`
    /// for a global order), returns what was lost, and starts a new sampling window.
    pub fn drain(&self, out: &mut Vec<Event>) -> Lost {
        let mut lost = Lost::default();
        let mut rings = GLOBAL.rings.lock().unwrap_or_else(PoisonError::into_inner);
        for shared in rings.iter() {
            while let Some(event) = shared.ring.pop() {
                out.push(event);
            }
            lost.dropped += shared.dropped.swap(0, Ordering::Relaxed);
            for (slot, count) in &shared.sampled {
                let key = slot.load(Ordering::Relaxed);
                let n = count.swap(0, Ordering::Relaxed);
                if key == 0 || n == 0 {
                    continue;
                }
                let code = Code((key - 1) as u16);
                match lost.sampled.iter_mut().find(|(c, _)| *c == code) {
                    Some((_, total)) => *total += n,
                    None => lost.sampled.push((code, n)),
                }
            }
        }
        // Threads that exited leave their ring behind; forget it once it is empty.
        rings.retain(|s| Arc::strong_count(s) > 1 || s.ring.len() > 0);
        drop(rings);
        GLOBAL.window.fetch_add(1, Ordering::Relaxed);
        lost
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        if GLOBAL.consumers.fetch_sub(1, Ordering::SeqCst) == 1 {
            GLOBAL.enabled.store(false, Ordering::SeqCst);
        }
    }
}

/// Emits a diagnostic built from a format string, checking [`admit`] first so a
/// sampled-out call never formats anything:
/// `ara_diag::emit!(warning, codes::FRAME_OVER_BUDGET, "frame {n} took {ms:.1} ms")`.
/// Evaluates to the event's `Option<EventId>`.
#[macro_export]
macro_rules! emit {
    ($severity:ident, $code:expr, $($arg:tt)+) => {{
        let code: $crate::Code = $code;
        if $crate::bus::admit(code) {
            $crate::bus::push(
                $crate::Diagnostic::$severity(code, ::std::format!($($arg)+)),
                $crate::bus::Meta::default(),
            )
        } else {
            ::std::option::Option::None
        }
    }};
}
