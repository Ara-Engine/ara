//! Light transport data for 25 cm surface patches (council-v3 architecture D2-D5):
//! patch charts, cached links, reverse links and the change feed that drives event
//! updates. No solver lives here yet (wave 2 adds `solve` and `media`).
//!
//! Ownership: `patches`, `links`, `reverse` belong to the A2 track; `changes` to A3;
//! `solve` and `media` to B3.
pub(crate) use ara_frame::change_feed as changes;
pub(crate) mod links;
pub(crate) mod media;
pub(crate) mod patches;
pub(crate) mod reverse;
pub(crate) mod solve;
