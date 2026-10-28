//! Derived probe work crosses typed channels; only the application owns renderer calls.
mod bridge;
mod params;
mod service;

pub(crate) use bridge::{Bridge, Event, Request, SceneBinding};
pub(crate) use service::ProbeService;

use std::sync::mpsc::sync_channel;

const CHANNEL_CAPACITY: usize = 128;
const MAX_JOBS: usize = 64;
const MAX_ENTITIES: usize = 256;

/// Connects the protocol service to the application's render-boundary owner.
pub(crate) fn channel() -> (ProbeService, Bridge) {
    let (requests, request_rx) = sync_channel(CHANNEL_CAPACITY);
    let (events, event_rx) = sync_channel(CHANNEL_CAPACITY);
    (
        ProbeService::new(requests, event_rx),
        Bridge::new(request_rx, events),
    )
}

#[cfg(test)]
mod tests;
