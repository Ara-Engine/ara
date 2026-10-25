//! Ara Hub (D25): the native launcher on the editor's UI stack. Projects, create
//! from templates and samples, engine installs with per-project pins, agent
//! onboarding and live activity, all over the user Hub service's JSON-RPC API.
//!
//! [`model::Hub`] is the whole UI state, driven by [`model::Intent`]s and
//! [`connection::Event`]s; [`view::page`] turns it into `ara_ui` markup;
//! [`native::run`] hosts it in an SDL3/Vulkan window.
pub mod capture;
pub mod connection;
pub mod model;
pub mod native;
pub mod view;
/// Source identity compiled into this Hub, independent of mutable installation metadata.
pub fn build_identity() -> Option<ara_update::BuildIdentity> {
    let commit = env!("ARA_HUB_COMMIT");
    let generation = env!("ARA_HUB_GENERATION").parse().ok()?;
    (!commit.is_empty()).then(|| ara_update::BuildIdentity {
        commit: commit.into(),
        generation,
    })
}
