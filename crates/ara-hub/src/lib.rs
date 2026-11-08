//! Ara Hub (D25): the native project launcher, on stillkit like the editor (D31). Projects,
//! create from templates and samples, engine installs with per-project pins, agent
//! onboarding and live activity, all over the user Hub service's JSON-RPC API.
//!
//! [`model::Hub`] is the whole UI state, driven by [`model::Intent`]s and
//! [`connection::Event`]s; [`ui::view`] turns it into stillkit elements;
//! [`native::run`] hosts it in one CPU-drawn, idle-free window.
pub mod capture;
pub mod connection;
pub mod model;
pub mod native;
pub mod ui;
/// Source identity compiled into this Hub, independent of mutable installation metadata.
pub fn build_identity() -> Option<ara_update::BuildIdentity> {
    let commit = env!("ARA_HUB_COMMIT");
    let generation = env!("ARA_HUB_GENERATION").parse().ok()?;
    (!commit.is_empty()).then(|| ara_update::BuildIdentity {
        commit: commit.into(),
        generation,
    })
}
