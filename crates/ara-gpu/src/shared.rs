//! Shared surfaces: a producer process (a play client) renders into a ring of
//! exportable images; the owner process (the editor) imports them on its own device
//! of the same physical GPU and samples them as textures, with no CPU copy (D19).
//!
//! The protocol, per slot of a [`SLOTS`]-image ring:
//! - The producer writes a free slot ([`Ring::begin`]), waits for that GPU work and
//!   sends a [`FrameReady`]. From then on the slot belongs to the owner.
//! - The owner keeps at most one unshown frame per ring ([`Mailbox`]): a newer frame
//!   supersedes it and the superseded one is released at once (it was never sampled).
//!   The shown frame is released when the next one is shown and the owner's GPU no
//!   longer samples it.
//! - Releases name the whole [`Frame`] (generation, slot, serial); stale or foreign
//!   ones are ignored, so a late release can never free a slot the owner holds again.
//!
//! The owner holds at most two slots, so the producer has a free one whenever the
//! owner keeps up; otherwise it skips rendering (never blocking the simulation) until
//! a release arrives. That adds at most one owner frame of latency. Between owners,
//! images rest in `TRANSFER_SRC_OPTIMAL`, owned by `VK_QUEUE_FAMILY_EXTERNAL`
//! ([`crate::vulkan::external`]). Handles are unnamed NT handles the owner pulls
//! out of the producer with `DuplicateHandle` ([`SharedSurface::duplicate_from`]).
use serde::{Deserialize, Serialize};

/// Images per ring: one shown, one pending and one being written.
pub const SLOTS: usize = 3;

/// Pixel format of shared surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SurfaceFormat {
    /// `R8G8B8A8_UNORM` holding the renderer's tonemapped, sRGB-encoded SDR output:
    /// sample it as UNORM (no decode), like the editor's own viewport image.
    Rgba8Unorm,
}

/// Physical GPU and driver identity. Opaque external memory imports only on a
/// device with the same device and driver UUIDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    /// `VkPhysicalDeviceIDProperties::deviceUUID`.
    pub device_uuid: [u8; 16],
    /// `VkPhysicalDeviceIDProperties::driverUUID`.
    pub driver_uuid: [u8; 16],
    /// The Windows adapter LUID, when the driver reports a valid one.
    pub luid: Option<[u8; 8]>,
}

/// One exported slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotDesc {
    /// Raw NT handle value, valid only inside [`SurfaceDesc::process`].
    pub handle: u64,
    /// Allocation size in bytes; an import must allocate exactly this much.
    pub size: u64,
    /// Memory type the producer allocated from; opaque imports must use the same.
    pub memory_type: u32,
}

/// One ring generation as described across the process boundary. Resizing makes a
/// new generation (new images, new handles); frames carry the generation they show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceDesc {
    /// Starts at 1 and increases on every resize.
    pub generation: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format of every slot.
    pub format: SurfaceFormat,
    /// The GPU and driver the images live on.
    pub device: DeviceIdentity,
    /// Process ID in which the slot handles are valid.
    pub process: u32,
    /// Exactly [`SLOTS`] slots.
    pub slots: Vec<SlotDesc>,
}

/// A completed frame: which slot of which generation, and the tick it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// Ring generation.
    pub generation: u64,
    /// Slot index in `0..SLOTS`.
    pub slot: u32,
    /// Increases with every published frame of the producer (across generations).
    pub serial: u64,
    /// Simulation tick the frame shows.
    pub tick: u64,
}

/// Producer → owner notice, sent after the frame's GPU work completed (a fence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameReady {
    /// The published frame.
    pub frame: Frame,
    /// Wall clock (Unix microseconds) when rendering completed, before the copy into
    /// the slot: owners subtract it from their receive time for the frame latency.
    pub rendered_at_us: u64,
    /// Producer time of the whole frame (prepare, render and copy, each waited for),
    /// in microseconds.
    pub cost_us: u64,
    /// Of which: copying into the slot and waiting for it, in microseconds.
    pub publish_us: u64,
}

/// Wall clock in Unix microseconds (comparable across processes on one machine).
pub fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Writing,
    /// Published with this serial; the owner holds it until released.
    Owned(u64),
}

/// The producer's slot states for one generation.
#[derive(Debug, Clone)]
pub struct Ring {
    generation: u64,
    slots: [State; SLOTS],
    next: usize,
}

impl Ring {
    /// A ring of free slots.
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            slots: [State::Free; SLOTS],
            next: 0,
        }
    }

    /// This ring's generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Slots the producer may write now.
    pub fn free(&self) -> usize {
        self.slots.iter().filter(|s| **s == State::Free).count()
    }

    /// Claims a free slot to write (round robin), or `None` when the owner holds
    /// every other slot: skip this frame and try again after a release.
    pub fn begin(&mut self) -> Option<u32> {
        let slot = (0..SLOTS)
            .map(|i| (self.next + i) % SLOTS)
            .find(|&i| self.slots[i] == State::Free)?;
        self.slots[slot] = State::Writing;
        self.next = (slot + 1) % SLOTS;
        Some(slot as u32)
    }

    /// Hands a written slot to the owner as `serial` showing `tick`.
    pub fn publish(&mut self, slot: u32, serial: u64, tick: u64) -> Frame {
        if let Some(state) = self.slots.get_mut(slot as usize) {
            *state = State::Owned(serial);
        }
        Frame {
            generation: self.generation,
            slot,
            serial,
            tick,
        }
    }

    /// Writing failed: the slot is free again.
    pub fn abandon(&mut self, slot: u32) {
        if let Some(state @ State::Writing) = self.slots.get_mut(slot as usize) {
            *state = State::Free;
        }
    }

    /// The owner returned `frame`. Returns false (and changes nothing) for another
    /// generation or a slot that no longer holds that serial.
    pub fn release(&mut self, frame: Frame) -> bool {
        match self.slots.get_mut(frame.slot as usize) {
            Some(state)
                if frame.generation == self.generation && *state == State::Owned(frame.serial) =>
            {
                *state = State::Free;
                true
            }
            _ => false,
        }
    }
}

/// The owner's side of one ring: the newest frame it has not shown yet.
#[derive(Debug, Clone, Default)]
pub struct Mailbox {
    pending: Option<Frame>,
}

impl Mailbox {
    /// A frame arrived. Returns the frame the owner must release right away: the
    /// superseded pending one (never sampled), or `frame` itself when it is older.
    pub fn deliver(&mut self, frame: Frame) -> Option<Frame> {
        match self.pending {
            Some(old) if newer(old, frame) => Some(frame),
            _ => self.pending.replace(frame),
        }
    }

    /// The newest unshown frame, if any.
    pub fn latest(&self) -> Option<Frame> {
        self.pending
    }

    /// Takes the newest unshown frame to show it; the caller now holds it until it
    /// releases it (after its GPU stopped sampling it).
    pub fn take(&mut self) -> Option<Frame> {
        self.pending.take()
    }
}

/// Whether `a` is newer than `b` (generation first, then serial).
fn newer(a: Frame, b: Frame) -> bool {
    (a.generation, a.serial) > (b.generation, b.serial)
}

/// A surface whose slot handles this process owns (closed on drop): what the owner
/// imports. Its [`SurfaceDesc`] names this process and its own handle values.
#[derive(Debug)]
pub struct SharedSurface {
    desc: SurfaceDesc,
    #[cfg(windows)]
    handles: Vec<std::os::windows::io::OwnedHandle>,
}

impl SharedSurface {
    /// The surface, with handle values valid in this process.
    pub fn desc(&self) -> &SurfaceDesc {
        &self.desc
    }

    /// Duplicates `desc`'s handles out of the producer `child` into this process.
    ///
    /// Security: the producer exports unnamed NT handles, so nothing can open them by
    /// name; only a process holding `PROCESS_DUP_HANDLE` on the producer can copy
    /// them. The owner already holds its child's full-access process handle, so the
    /// child never needs a handle to the owner (it cannot push handles into or act on
    /// the editor process). Unsupported (an error) outside Windows.
    pub fn duplicate_from(
        child: &std::process::Child,
        desc: &SurfaceDesc,
    ) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            Self::duplicate(child.as_raw_handle(), desc)
        }
        #[cfg(not(windows))]
        {
            let _ = (child, desc);
            Err(unsupported())
        }
    }

    /// Duplicates handles that are valid in this process (the producer and owner are
    /// one process, as in tests).
    pub fn duplicate_local(desc: &SurfaceDesc) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            // SAFETY: GetCurrentProcess returns a constant pseudo-handle; no ownership.
            let process = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
            Self::duplicate(process, desc)
        }
        #[cfg(not(windows))]
        {
            let _ = desc;
            Err(unsupported())
        }
    }

    /// A second owner of the same memory: every handle duplicated (closed separately).
    pub fn try_clone(&self) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let handles = self
                .handles
                .iter()
                .map(std::os::windows::io::OwnedHandle::try_clone)
                .collect::<std::io::Result<Vec<_>>>()?;
            let mut desc = self.desc.clone();
            for (slot, handle) in desc.slots.iter_mut().zip(&handles) {
                slot.handle = handle.as_raw_handle() as u64;
            }
            Ok(Self { desc, handles })
        }
        #[cfg(not(windows))]
        {
            Err(unsupported())
        }
    }

    #[cfg(windows)]
    fn duplicate(
        process: std::os::windows::io::RawHandle,
        desc: &SurfaceDesc,
    ) -> std::io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE},
            System::Threading::GetCurrentProcess,
        };
        if desc.slots.len() != SLOTS {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "a shared surface has {SLOTS} slots, not {}",
                    desc.slots.len()
                ),
            ));
        }
        let mut handles = Vec::with_capacity(SLOTS);
        for slot in &desc.slots {
            let mut out: HANDLE = std::ptr::null_mut();
            // SAFETY: `process` is a live process handle with PROCESS_DUP_HANDLE access
            // (a spawned child's handle or the current-process pseudo-handle) and the
            // source value is a handle in that process; failure leaves `out` null.
            let ok = unsafe {
                DuplicateHandle(
                    process as HANDLE,
                    slot.handle as HANDLE,
                    GetCurrentProcess(),
                    &mut out,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            };
            if ok == 0 || out.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: DuplicateHandle succeeded: `out` is a new handle this process owns.
            handles.push(unsafe { OwnedHandle::from_raw_handle(out.cast()) });
        }
        let mut desc = desc.clone();
        desc.process = std::process::id();
        for (slot, handle) in desc.slots.iter_mut().zip(&handles) {
            slot.handle = handle.as_raw_handle() as u64;
        }
        Ok(Self { desc, handles })
    }
}

#[cfg(not(windows))]
fn unsupported() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "shared surfaces need Windows NT handles on this backend",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(generation: u64, slot: u32, serial: u64) -> Frame {
        Frame {
            generation,
            slot,
            serial,
            tick: serial,
        }
    }

    #[test]
    fn ring_cycles_free_slots_and_skips_when_the_owner_holds_the_rest() {
        let mut ring = Ring::new(1);
        let a = ring.begin().unwrap();
        let fa = ring.publish(a, 1, 10);
        let b = ring.begin().unwrap();
        let fb = ring.publish(b, 2, 11);
        let c = ring.begin().unwrap();
        assert_eq!([a, b, c], [0, 1, 2]);
        assert_eq!(ring.begin(), None, "every slot is written or owned");
        ring.abandon(c);
        assert_eq!(ring.free(), 1);
        assert!(ring.release(fa));
        assert!(!ring.release(fa), "a duplicate release is ignored");
        assert!(
            !ring.release(Frame { serial: 9, ..fb }),
            "a stale serial is ignored"
        );
        assert!(
            !ring.release(Frame {
                generation: 2,
                ..fb
            }),
            "another generation"
        );
        assert_eq!(ring.free(), 2);
        // Round robin continues after the last claimed slot.
        assert_eq!(ring.begin(), Some(0));
    }

    #[test]
    fn mailbox_keeps_the_newest_and_returns_what_to_release() {
        let mut mailbox = Mailbox::default();
        assert_eq!(mailbox.deliver(frame(1, 0, 1)), None);
        assert_eq!(mailbox.deliver(frame(1, 1, 2)), Some(frame(1, 0, 1)));
        assert_eq!(
            mailbox.deliver(frame(1, 0, 1)),
            Some(frame(1, 0, 1)),
            "older"
        );
        assert_eq!(mailbox.latest(), Some(frame(1, 1, 2)));
        assert_eq!(mailbox.take(), Some(frame(1, 1, 2)));
        assert_eq!(mailbox.take(), None);
        assert_eq!(mailbox.deliver(frame(2, 0, 3)), None);
    }

    /// Producer and owner exchange messages through queues with arbitrary delays:
    /// the producer never writes a slot the owner holds, the owner never holds more
    /// than two, and frames keep flowing.
    #[test]
    fn protocol_never_writes_a_held_slot_and_never_stalls() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut random = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut ring = Ring::new(1);
        let mut serial = 0;
        let mut to_owner: std::collections::VecDeque<Frame> = Default::default();
        let mut to_producer: std::collections::VecDeque<Frame> = Default::default();
        let mut mailbox = Mailbox::default();
        let mut shown: Option<Frame> = None;
        // What the owner holds right now (pending, shown, or in flight to it).
        let held = |to_owner: &std::collections::VecDeque<Frame>,
                    mailbox: &Mailbox,
                    shown: Option<Frame>| {
            let mut slots: Vec<u32> = to_owner.iter().map(|f| f.slot).collect();
            slots.extend(mailbox.latest().map(|f| f.slot));
            slots.extend(shown.map(|f| f.slot));
            slots
        };
        let mut published = 0;
        let mut shows = 0;
        for _ in 0..20_000 {
            match random(4) {
                0 => {
                    if let Some(slot) = ring.begin() {
                        let holding = held(&to_owner, &mailbox, shown);
                        assert!(
                            !holding.contains(&slot),
                            "wrote slot {slot} the owner holds"
                        );
                        serial += 1;
                        to_owner.push_back(ring.publish(slot, serial, serial));
                        published += 1;
                    }
                }
                1 => {
                    if let Some(f) = to_owner.pop_front() {
                        to_producer.extend(mailbox.deliver(f));
                    }
                }
                2 => {
                    if let Some(f) = mailbox.take() {
                        // The previous frame's sampling completed: release it.
                        to_producer.extend(shown.replace(f));
                        shows += 1;
                    }
                }
                _ => {
                    if let Some(f) = to_producer.pop_front() {
                        assert!(ring.release(f));
                    }
                }
            }
            let owner = mailbox.latest().is_some() as usize + shown.is_some() as usize;
            assert!(owner <= 2);
        }
        assert!(
            published > 1000 && shows > 200,
            "{published} published, {shows} shown"
        );
    }

    #[test]
    fn descriptions_round_trip_as_camel_case_json() {
        let desc = SurfaceDesc {
            generation: 2,
            width: 640,
            height: 360,
            format: SurfaceFormat::Rgba8Unorm,
            device: DeviceIdentity {
                device_uuid: [1; 16],
                driver_uuid: [2; 16],
                luid: Some([3; 8]),
            },
            process: 42,
            slots: vec![
                SlotDesc {
                    handle: 7,
                    size: 1 << 20,
                    memory_type: 1,
                };
                SLOTS
            ],
        };
        let json = serde_json::to_value(&desc).unwrap();
        assert_eq!(json["format"], "rgba8Unorm");
        assert_eq!(json["slots"][0]["memoryType"], 1);
        assert_eq!(serde_json::from_value::<SurfaceDesc>(json).unwrap(), desc);
    }
}
