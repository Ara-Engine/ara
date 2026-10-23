//! Animation controllers authored in `.animation` documents (`Animator.controller`):
//! state machines, blend trees and motion matching over the entity model's clips.
//!
//! References name records of `.animation` documents (`@anim/fox.animation#an_walk`;
//! the kernel keeps record references, see `SchemaDb::address_records`), so one file
//! holds a character's clips and controllers. A reference to a document ID names its
//! only record. A clip record with source keys (`clip`) is cooked as authored; one
//! without keys binds the model animation whose name is the record's `name`
//! (`{"name": "Walk"}` → the glTF's `Walk`). A motion-matching node's `clip` names the
//! record holding the `motionDatabase` configuration; the database is cooked from its
//! clips when the program compiles (tick boundary).
use crate::{
    Result, RuntimeError,
    model::Model,
    skin::{Skin, TICKS_PER_SECOND},
};
use ara_anim::{
    CompressionSettings, Controller, CookedClip, Pose,
    document::AnimAsset,
    glam::{Quat, Vec3},
    graph::{NodeKind, ParameterId, ParameterValue},
    matching::{FeatureWriter, MatchConfig, MotionDatabase, TrajectoryPoint, TrajectoryPredictor},
};
use ara_doc::Workspace;
use ara_schema::AssetRef;
use std::collections::BTreeMap;

/// Every `AnimAsset` record of the project's `animation` documents, by (document
/// path, record ID); documents by ID with their path and record IDs.
#[derive(Default)]
pub struct Documents {
    records: BTreeMap<(String, String), AnimAsset>,
    documents: BTreeMap<String, (String, Vec<String>)>,
}

impl Documents {
    /// Decodes the records of every `animation` document in `workspace`.
    pub fn load(workspace: &Workspace) -> Result<Self> {
        let mut records = BTreeMap::new();
        let mut documents = BTreeMap::new();
        for (path, doc) in workspace.docs() {
            if doc.kind != "animation" {
                continue;
            }
            let mut ids = Vec::new();
            for id in doc.records().keys() {
                let record = ara_anim::document::asset(doc, id)
                    .map_err(|e| RuntimeError(format!("{path}#{id}: {}", e.0)))?;
                records.insert((path.to_owned(), id.to_string()), record);
                ids.push(id.to_string());
            }
            documents.insert(doc.id.to_string(), (path.to_owned(), ids));
        }
        Ok(Self { records, documents })
    }

    /// The record a reference names: `@anim/fox.animation#an_walk`, or the only
    /// record of a document named by its ID (`@anim/walk.animation#ac_walk0001`).
    pub fn record(&self, reference: &AssetRef) -> Result<&AnimAsset> {
        let parts = ara_schema::grammar::parse_ref(&reference.0).map_err(RuntimeError)?;
        let id = parts.id.unwrap_or_default();
        let key = match self.documents.get(id) {
            Some((path, ids)) => match ids.as_slice() {
                [only] => (path.clone(), only.clone()),
                _ => {
                    return Err(RuntimeError(format!(
                        "{} names a document with several records; name one (`@path#an_…`)",
                        reference.0
                    )));
                }
            },
            None => (parts.path.unwrap_or_default().to_owned(), id.to_owned()),
        };
        self.records
            .get(&key)
            .ok_or_else(|| RuntimeError(format!("no animation record {}", reference.0)))
    }
}

/// Motion-matching inputs of a compiled program.
pub struct MatchProgram {
    /// Feature layout of the database (bones, horizons, weights).
    pub config: MatchConfig,
    /// The motion node's float query parameters, in raw feature order.
    pub query: Vec<ParameterId>,
    /// Trajectory horizons in seconds.
    pub horizons: Vec<f32>,
    /// Predictor spring half-life in seconds.
    pub half_life: f32,
}

/// A program compiled from an `.animation` controller.
pub struct Compiled {
    /// The immutable ara-anim program.
    pub controller: Controller,
    /// Motion matching, when the controller has a motion-matching node.
    pub matching: Option<MatchProgram>,
}

fn cook(skin: &Skin, documents: &Documents, reference: &AssetRef) -> Result<CookedClip> {
    let record = documents.record(reference)?;
    let data = match &record.clip {
        Some(data) => data,
        None => skin.clips.get(&record.name).ok_or_else(|| {
            RuntimeError(format!(
                "{}: the model has no animation named `{}`",
                reference.0, record.name
            ))
        })?,
    };
    CookedClip::cook(data, &skin.skeleton, CompressionSettings::default())
        .map(|(cooked, _)| cooked)
        .map_err(|e| RuntimeError(format!("clip {}: {}", reference.0, e.0)))
}

/// Compiles the controller of `reference` against `model`'s first skin at `hz`. The
/// document's `fixedStep` is replaced by the simulation's (clip timebase / `hz`).
pub fn compile(
    model: &Model,
    documents: &Documents,
    reference: &AssetRef,
    hz: u32,
) -> Result<Compiled> {
    let skin = model
        .skins
        .first()
        .ok_or_else(|| RuntimeError(format!("{} has no skin", model.asset)))?;
    let mut data = documents
        .record(reference)?
        .controller
        .clone()
        .ok_or_else(|| RuntimeError(format!("{} has no controller", reference.0)))?;
    data.fixed_step = TICKS_PER_SECOND / hz;
    let mut bindings: Vec<(AssetRef, CookedClip)> = Vec::new();
    let mut databases = Vec::new();
    let mut matching = None;
    for node in data.nodes.values() {
        let Some(clip) = &node.clip else { continue };
        if node.kind == NodeKind::MotionMatching {
            let config = documents
                .record(clip)?
                .motion_database
                .clone()
                .ok_or_else(|| RuntimeError(format!("{} has no motionDatabase", clip.0)))?;
            let cooked = config
                .clips
                .iter()
                .map(|c| cook(skin, documents, c).map(|k| (c.clone(), k)))
                .collect::<Result<Vec<_>>>()?;
            let borrowed: Vec<_> = cooked.iter().map(|(r, k)| (r.clone(), k)).collect();
            let database = MotionDatabase::cook(&config, &skin.skeleton, &borrowed)
                .map_err(|e| RuntimeError(format!("motion database {}: {}", clip.0, e.0)))?;
            databases.push((clip.clone(), database));
            for (r, k) in cooked {
                if !bindings.iter().any(|(b, _)| *b == r) {
                    bindings.push((r, k));
                }
            }
            let node_matching = node.matching.clone().unwrap_or_default();
            matching = Some((config, node_matching));
        } else if !bindings.iter().any(|(b, _)| b == clip) {
            bindings.push((clip.clone(), cook(skin, documents, clip)?));
        }
    }
    let controller = Controller::compile_with_matching(&data, &skin.skeleton, bindings, databases)
        .map_err(|e| RuntimeError(format!("{}: {}", reference.0, e.0)))?;
    let matching = match matching {
        Some((config, node)) => {
            let query = node
                .query
                .iter()
                .map(|name| {
                    controller
                        .parameter(name)
                        .ok_or_else(|| RuntimeError(format!("missing query parameter {name}")))
                })
                .collect::<Result<Vec<_>>>()?;
            if query.len() != config.dimensions() {
                return Err(RuntimeError(format!(
                    "{}: {} query parameters for {} features",
                    reference.0,
                    query.len(),
                    config.dimensions()
                )));
            }
            Some(MatchProgram {
                horizons: config
                    .horizons
                    .iter()
                    .map(|t| *t as f32 / TICKS_PER_SECOND as f32)
                    .collect(),
                config,
                query,
                half_life: node.half_life.max(0.05),
            })
        }
        None => None,
    };
    Ok(Compiled {
        controller,
        matching,
    })
}

/// Per-actor motion-matching state: the predictor and the last two end-of-tick poses
/// (feature positions and velocities) are authoritative and snapshotted
/// ([`MatchSnapshot`]); the writer, features and trajectory are scratch.
pub struct MatchState {
    /// Character locomotion predictor.
    pub predictor: TrajectoryPredictor,
    current: Pose,
    previous: Pose,
    writer: FeatureWriter,
    features: Vec<f32>,
    trajectory: Vec<TrajectoryPoint>,
}

/// Portable [`MatchState`]: predictor plus the two feature poses as
/// `[tx, ty, tz, rx, ry, rz, rw, sx, sy, sz]` per joint.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MatchSnapshot {
    /// Predictor state.
    pub predictor: TrajectoryPredictor,
    /// Pose after the last tick.
    pub current: Vec<[f32; 10]>,
    /// Pose after the tick before.
    pub previous: Vec<[f32; 10]>,
}

fn pose_data(pose: &Pose) -> Vec<[f32; 10]> {
    (0..pose.len())
        .filter_map(|i| pose.joint(i))
        .map(|t| {
            let (tr, r, s) = (t.translation, t.rotation, t.scale);
            [tr.x, tr.y, tr.z, r.x, r.y, r.z, r.w, s.x, s.y, s.z]
        })
        .collect()
}

fn set_pose(pose: &mut Pose, data: &[[f32; 10]]) -> std::result::Result<(), ara_anim::Error> {
    for (i, d) in data.iter().enumerate() {
        pose.set_joint(
            i,
            ara_anim::Transform {
                translation: Vec3::new(d[0], d[1], d[2]).into(),
                rotation: Quat::from_xyzw(d[3], d[4], d[5], d[6]),
                scale: Vec3::new(d[7], d[8], d[9]).into(),
            },
        )?;
    }
    Ok(())
}

impl MatchState {
    /// Allocates scratch for `program` at the actor's position and yaw; both feature
    /// poses start at the animator's initial pose.
    pub fn new(
        program: &MatchProgram,
        skin: &Skin,
        animator: &ara_anim::Animator,
        position: Vec3,
        yaw: f32,
    ) -> Result<Self> {
        let mut current = Pose::new(skin.skeleton.len());
        current
            .copy_from(animator.pose())
            .map_err(|e| RuntimeError(e.0))?;
        let mut previous = Pose::new(skin.skeleton.len());
        previous
            .copy_from(&current)
            .map_err(|e| RuntimeError(e.0))?;
        Ok(Self {
            predictor: TrajectoryPredictor {
                position,
                yaw,
                ..Default::default()
            },
            current,
            previous,
            writer: FeatureWriter::new(&program.config, &skin.skeleton)
                .map_err(|e| RuntimeError(e.0))?,
            features: vec![0.0; program.config.dimensions()],
            trajectory: vec![TrajectoryPoint::default(); program.horizons.len()],
        })
    }

    /// Advances the predictor toward `velocity` (world, m/s) and writes the query
    /// features (from the last two end-of-tick poses) into the animator parameters.
    pub fn query(
        &mut self,
        program: &MatchProgram,
        skin: &Skin,
        animator: &mut ara_anim::Animator,
        velocity: Vec3,
        dt: f32,
    ) -> std::result::Result<(), ara_anim::Error> {
        let planar = Vec3::new(velocity.x, 0.0, velocity.z);
        let yaw = if planar.length_squared() > 1e-4 {
            planar.x.atan2(planar.z)
        } else {
            self.predictor.yaw
        };
        self.predictor.step(planar, yaw, program.half_life, dt)?;
        self.predictor.predict(
            planar,
            yaw,
            program.half_life,
            &program.horizons,
            dt,
            &mut self.trajectory,
        )?;
        let local = Quat::from_rotation_y(-self.predictor.yaw) * self.predictor.motion.value;
        self.writer.write(
            &skin.skeleton,
            &self.current,
            &self.previous,
            dt,
            local,
            &self.trajectory,
            &mut self.features,
        )?;
        for (id, value) in program.query.iter().zip(&self.features) {
            animator.set(*id, ParameterValue::Float(*value))?;
        }
        Ok(())
    }

    /// Records the animator pose after a tick (the next query reads it).
    pub fn observe(&mut self, pose: &Pose) -> std::result::Result<(), ara_anim::Error> {
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.copy_from(pose)
    }

    /// Portable state for snapshots.
    pub fn snapshot(&self) -> MatchSnapshot {
        MatchSnapshot {
            predictor: self.predictor,
            current: pose_data(&self.current),
            previous: pose_data(&self.previous),
        }
    }

    /// Restores [`MatchState::snapshot`] output.
    pub fn restore(
        &mut self,
        snapshot: &MatchSnapshot,
    ) -> std::result::Result<(), ara_anim::Error> {
        self.predictor = snapshot.predictor;
        set_pose(&mut self.current, &snapshot.current)?;
        set_pose(&mut self.previous, &snapshot.previous)
    }
}
