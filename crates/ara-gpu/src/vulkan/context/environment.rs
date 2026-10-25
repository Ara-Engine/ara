//! Machine notices shared by every Vulkan instance in this process.
use std::{
    collections::BTreeSet,
    sync::{Mutex, OnceLock},
};

/// Only GENERAL notices enter here. Validation/performance diagnostics bypass
/// admission entirely, so a repeated hazard remains counted and reportable.
pub(super) fn first(message: &str) -> bool {
    static SEEN: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    SEEN.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(message.to_owned())
}

pub(super) fn metadata(message: &str) -> ara_diag::bus::Meta {
    ara_diag::bus::Meta::default().dedup(&format!("GPU environment: {message}"))
}
