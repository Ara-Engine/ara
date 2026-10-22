//! Ara-owned declarations. No transport types escape this module's consumers.

/// Who is allowed to receive a component's wire representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    /// Every relevant connection (the spike uses one room).
    Relevant,
    /// Only the authenticated input owner.
    OwnerOnly,
}

/// Bounded wire precision, independent of simulation history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantization {
    /// Integer millimetres; simulation already uses this representation.
    Millimetres,
    /// Exact bounded integers or flags.
    Exact,
}

/// Remote presentation rule; never writes simulation state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// Buffer two checkpoints and linearly sample presentation.
    Linear,
    /// Apply authoritative discrete state.
    None,
}

/// Backend-independent metadata consumed by the adapter.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Stable schema identity, versioned before shipping.
    pub id: u16,
    /// Human-readable diagnostics name.
    pub name: &'static str,
    /// Recipient rule enforced before encoding.
    pub audience: Audience,
    /// Wire precision.
    pub quantization: Quantization,
    /// Presentation policy.
    pub interpolation: Interpolation,
    /// Whether the complete component is restored and resimulated.
    pub rollback: bool,
}

/// One declaration supplies component registration and policy.
pub trait NetSchema {
    /// Registration metadata, independent of the chosen backend.
    const POLICY: Policy;
}

/// An authenticated tick-boundary intent, never an editor document edit.
pub trait IntentSchema {
    /// Stable schema identity, separate from transport message discriminants.
    const ID: u16;
    /// Name used in rejection diagnostics and generated tools.
    const NAME: &'static str;
    /// Maximum accepted commands per second per caller.
    const RATE_PER_SECOND: u16;
}

macro_rules! component {
    (#[net($id:literal, $aud:ident, $quant:ident, $interp:ident, $rollback:literal)]
     $(#[$meta:meta])* $vis:vis struct $name:ident { $($(#[$fm:meta])* $fv:vis $field:ident : $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(bevy_ecs::prelude::Component, serde::Serialize, serde::Deserialize,
                 Clone, Copy, Debug, Default, PartialEq, Eq)]
        $vis struct $name { $($(#[$fm])* $fv $field: $ty),* }
        impl $crate::schema::NetSchema for $name {
            const POLICY: $crate::schema::Policy = $crate::schema::Policy {
                id: $id, name: stringify!($name),
                audience: $crate::schema::Audience::$aud,
                quantization: $crate::schema::Quantization::$quant,
                interpolation: $crate::schema::Interpolation::$interp,
                rollback: $rollback,
            };
        }
    };
}
pub(crate) use component;

macro_rules! intent {
    (#[intent($id:literal, rate = $rate:literal)] $(#[$meta:meta])* $vis:vis struct $name:ident {
        $($(#[$fm:meta])* $fv:vis $field:ident : $ty:ty),* $(,)?
    }) => {
        $(#[$meta])*
        #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
        $vis struct $name { $($(#[$fm])* $fv $field: $ty),* }
        impl $crate::schema::IntentSchema for $name {
            const ID: u16 = $id;
            const NAME: &'static str = stringify!($name);
            const RATE_PER_SECOND: u16 = $rate;
        }
    };
}
pub(crate) use intent;
