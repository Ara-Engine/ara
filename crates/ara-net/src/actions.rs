//! Optional multi-action codec for one complete predicted feature input.
use ara_input::ActionValue;
use ara_sim::SimError;
use bevy_ecs::prelude::Resource;
use std::collections::BTreeMap;

/// Validated action names and values expanded from a feature's native input.
pub type ExpandedActions = Vec<(String, ActionValue)>;

/// Converts several declared actions into one bounded native input-history entry.
/// Both peers install the same codec before connecting. Expansion validates every
/// received input and supplies the authority's replayable action boundary.
#[derive(Resource)]
pub struct ActionCodec<I: 'static> {
    /// Declared actions accepted by device and agent injection.
    pub names: &'static [&'static str],
    /// Samples the latest unexpired value of each action at a new prediction tick.
    pub sample: fn(&BTreeMap<String, ActionValue>) -> Result<I, SimError>,
    /// Validates typed wire input and expands it into declared actions.
    pub expand: fn(&I) -> Result<ExpandedActions, SimError>,
}
