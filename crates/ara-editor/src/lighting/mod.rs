//! What lights a scene, read and authored alike by both windows: the lights, sky, fog,
//! post-process volumes and reflection probes in one list with each volume's live weight
//! (`model`), the volumes, fog and probes Add creates and the look presets (`authoring`),
//! and local volumes' cages as the renderer bounds them (`bounds`).
pub(crate) mod authoring;
pub(crate) mod bounds;
pub(crate) mod model;

use crate::Editor;
use ara_components::post::{PostProcessVolume, PostVolume, VolumeShape};
use glam::Vec3;
use serde_json::{Value, json};
