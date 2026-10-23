use crate::matching::{MatchNode, MotionDatabase};
use crate::{
    CookedClip, Error, Skeleton, ensure,
    graph::{
        Comparison, ControllerData, Node, NodeKind, ParameterId, ParameterKind, ParameterValue,
    },
    signature,
};
use ara_schema::{AssetRef, grammar::parse_ref};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub(crate) struct ProgramNode {
    pub data: Node,
    pub children: Vec<usize>,
    pub clip: usize,
    pub x: usize,
    pub y: usize,
    pub duration: u32,
    pub matching: Option<ProgramMatch>,
}
#[derive(Debug)]
pub(crate) struct ProgramMatch {
    pub database: usize,
    pub clips: Vec<usize>,
    pub parameters: Vec<usize>,
    pub settings: MatchNode,
}
#[derive(Debug)]
pub(crate) struct ProgramState {
    pub node: usize,
    pub duration: u32,
    pub looping: bool,
    pub sync_group: String,
    pub plan: Vec<usize>,
}
#[derive(Debug)]
pub(crate) struct Predicate {
    pub parameter: usize,
    pub comparison: Comparison,
    pub value: ParameterValue,
}
impl Predicate {
    pub fn matches(&self, values: &[ParameterValue]) -> bool {
        let value = values[self.parameter];
        match self.comparison {
            Comparison::Equal => value == self.value,
            Comparison::NotEqual => value != self.value,
            Comparison::Trigger => value == ParameterValue::Trigger(true),
            Comparison::Greater => match (value, self.value) {
                (ParameterValue::Float(a), ParameterValue::Float(b)) => a > b,
                (ParameterValue::Int(a), ParameterValue::Int(b)) => a > b,
                _ => false,
            },
            Comparison::Less => match (value, self.value) {
                (ParameterValue::Float(a), ParameterValue::Float(b)) => a < b,
                (ParameterValue::Int(a), ParameterValue::Int(b)) => a < b,
                _ => false,
            },
        }
    }
}
#[derive(Debug)]
pub(crate) struct ProgramTransition {
    pub from: usize,
    pub to: usize,
    pub duration: u32,
    pub exit_tick: Option<u32>,
    pub conditions: Vec<Predicate>,
}

/// Immutable animation program. Share this resource across all ECS actors.
#[derive(Debug)]
pub struct Controller {
    pub(crate) signature: u64,
    pub(crate) skeleton: u64,
    pub(crate) joints: usize,
    pub(crate) root: usize,
    pub(crate) clips: Vec<CookedClip>,
    pub(crate) nodes: Vec<ProgramNode>,
    pub(crate) states: Vec<ProgramState>,
    pub(crate) transitions: Vec<ProgramTransition>,
    pub(crate) defaults: Vec<ParameterValue>,
    pub(crate) names: Vec<String>,
    pub(crate) properties: Vec<String>,
    pub(crate) curve_maps: Vec<Vec<usize>>,
    pub(crate) initial: usize,
    pub(crate) step: u32,
    pub(crate) extract: bool,
    pub(crate) databases: Vec<MotionDatabase>,
}
impl Controller {
    /// Resolves canonical clip references and validates the entire controller DAG.
    /// Unused nodes are checked too, so editor diagnostics do not depend on state.
    pub fn compile(
        data: &ControllerData,
        skeleton: &Skeleton,
        bindings: Vec<(AssetRef, CookedClip)>,
    ) -> Result<Self, Error> {
        Self::compile_with_matching(data, skeleton, bindings, Vec::new())
    }
    /// Compiles controllers containing motion-matching nodes. Database and clip
    /// generations are checked together before an immutable program is published.
    pub fn compile_with_matching(
        data: &ControllerData,
        skeleton: &Skeleton,
        bindings: Vec<(AssetRef, CookedClip)>,
        databases: Vec<(AssetRef, MotionDatabase)>,
    ) -> Result<Self, Error> {
        ensure(
            data.fixed_step > 0 && !data.states.is_empty() && data.nodes.len() <= 1024,
            "invalid controller size or fixed step",
        )?;
        let names: Vec<_> = data.parameters.keys().cloned().collect();
        let defaults: Vec<_> = data.parameters.values().map(|p| p.value()).collect();
        ensure(
            names.iter().all(|n| !n.is_empty()) && defaults.iter().all(|v| v.valid()),
            "invalid parameter name or default",
        )?;
        let mut clip_names = BTreeMap::new();
        let mut clips = Vec::with_capacity(bindings.len());
        for (reference, clip) in bindings {
            let parts = parse_ref(&reference.0).map_err(Error)?;
            ensure(
                parts.path.is_some() && parts.id.is_some(),
                "clip references require @path#id",
            )?;
            ensure(
                clip.skeleton == skeleton.signature()
                    && clips
                        .first()
                        .is_none_or(|first: &CookedClip| first.frequency == clip.frequency),
                "controller clips require the same skeleton and timebase",
            )?;
            ensure(
                clip_names.insert(reference.0, clips.len()).is_none(),
                "duplicate clip reference binding",
            )?;
            clips.push(clip);
        }
        let mut nodes = Vec::new();
        let mut database_names = BTreeMap::new();
        let mut database_values = Vec::new();
        for (name, db) in databases {
            let parts = parse_ref(&name.0).map_err(Error)?;
            ensure(
                parts.path.is_some()
                    && parts.id.is_some()
                    && db.skeleton == skeleton.signature()
                    && clips.first().is_some_and(|c| c.frequency == db.frequency),
                "motion database skeleton/timebase/reference mismatch",
            )?;
            for (reference, expected) in db.references.iter().zip(&db.clips) {
                ensure(
                    clip_names
                        .get(&reference.0)
                        .is_some_and(|i| clips[*i].signature == *expected),
                    "motion database uses a different clip generation",
                )?;
            }
            ensure(
                database_names
                    .insert(name.0, database_values.len())
                    .is_none(),
                "duplicate motion database reference",
            )?;
            database_values.push(db);
        }
        let mut resolved = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        for id in data.nodes.keys() {
            visit(
                id,
                data,
                &clip_names,
                &clips,
                &database_names,
                &database_values,
                skeleton.len(),
                &mut nodes,
                &mut resolved,
                &mut visiting,
            )?;
        }
        let state_names: BTreeMap<_, _> = data
            .states
            .keys()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        let initial = *state_names
            .get(data.initial.as_str())
            .ok_or_else(|| Error("initial state does not exist".into()))?;
        let mut states = Vec::new();
        for (id, state) in &data.states {
            ensure(!id.is_empty(), "empty state ID")?;
            let node = *resolved
                .get(&state.node)
                .ok_or_else(|| Error("state graph node does not exist".into()))?;
            let duration = if state.duration == 0 {
                nodes[node].duration
            } else {
                state.duration
            };
            ensure(duration > 0, "state duration must be nonzero")?;
            let mut reachable = vec![false; nodes.len()];
            reachable[node] = true;
            for i in (0..=node).rev() {
                if reachable[i] {
                    for child in &nodes[i].children {
                        reachable[*child] = true;
                    }
                }
            }
            states.push(ProgramState {
                node,
                duration,
                looping: state.looping,
                sync_group: state.sync_group.clone(),
                plan: (0..nodes.len()).filter(|i| reachable[*i]).collect(),
            });
        }
        let mut transitions = Vec::new();
        let mut transition_ids = BTreeSet::new();
        for t in &data.transitions {
            ensure(
                !t.id.is_empty() && transition_ids.insert(&t.id),
                "transition IDs must be nonempty and unique",
            )?;
            let from = *state_names
                .get(t.from.as_str())
                .ok_or_else(|| Error("transition source state does not exist".into()))?;
            let to = *state_names
                .get(t.to.as_str())
                .ok_or_else(|| Error("transition target state does not exist".into()))?;
            ensure(
                t.exit_tick.is_none_or(|tick| tick <= states[from].duration),
                "exit tick is past the state duration",
            )?;
            let mut conditions = Vec::new();
            for c in &t.conditions {
                let parameter = names
                    .binary_search(&c.parameter)
                    .map_err(|_| Error("condition parameter does not exist".into()))?;
                let kind = defaults[parameter].kind();
                let value = match kind {
                    ParameterKind::Float => ParameterValue::Float(c.float_value),
                    ParameterKind::Int => ParameterValue::Int(c.int_value),
                    ParameterKind::Bool => ParameterValue::Bool(c.bool_value),
                    ParameterKind::Trigger => ParameterValue::Trigger(true),
                };
                ensure(
                    value.valid()
                        && match c.comparison {
                            Comparison::Trigger => kind == ParameterKind::Trigger,
                            Comparison::Equal | Comparison::NotEqual => {
                                kind != ParameterKind::Trigger
                            }
                            Comparison::Greater | Comparison::Less => {
                                matches!(kind, ParameterKind::Float | ParameterKind::Int)
                            }
                        },
                    "condition comparison does not match parameter type",
                )?;
                conditions.push(Predicate {
                    parameter,
                    comparison: c.comparison,
                    value,
                });
            }
            transitions.push(ProgramTransition {
                from,
                to,
                duration: t.duration,
                exit_tick: t.exit_tick,
                conditions,
            });
        }
        let mut properties = BTreeSet::new();
        for clip in &clips {
            for curve in &clip.curves {
                properties.insert(curve.property.clone());
            }
        }
        let properties: Vec<_> = properties.into_iter().collect();
        let curve_maps = clips
            .iter()
            .map(|clip| {
                clip.curves
                    .iter()
                    .map(|c| {
                        properties
                            .binary_search(&c.property)
                            .expect("all clip properties were collected")
                    })
                    .collect()
            })
            .collect();
        let mut bytes = postcard::to_allocvec(data).map_err(|e| Error(e.to_string()))?;
        bytes.extend(skeleton.signature().to_le_bytes());
        for clip in &clips {
            bytes.extend(clip.signature.to_le_bytes());
        }
        for db in &database_values {
            bytes.extend(db.signature.to_le_bytes());
        }
        Ok(Self {
            signature: signature(&bytes),
            skeleton: skeleton.signature(),
            joints: skeleton.len(),
            root: skeleton.data.motion_root as usize,
            clips,
            nodes,
            states,
            transitions,
            defaults,
            names,
            properties,
            curve_maps,
            initial,
            step: data.fixed_step,
            extract: data.extract_root_motion,
            databases: database_values,
        })
    }
    /// Resolves a parameter name for cached typed gameplay access.
    pub fn parameter(&self, name: &str) -> Option<ParameterId> {
        self.names
            .binary_search_by(|n| n.as_str().cmp(name))
            .ok()
            .map(|i| ParameterId(i as u32))
    }
    /// Scalar property output paths in deterministic sorted order.
    pub fn curve_properties(&self) -> &[String] {
        &self.properties
    }
    /// Fixed tick interval in the clips' shared timebase.
    pub fn fixed_step(&self) -> u32 {
        self.step
    }
    /// Clip tick frequency used to convert the simulation's fixed interval.
    pub fn ticks_per_second(&self) -> u32 {
        self.clips[0].frequency
    }
    /// Content signature used to reject snapshots from another program generation.
    pub fn signature(&self) -> u64 {
        self.signature
    }
}

#[allow(clippy::too_many_arguments)]
fn visit(
    id: &str,
    data: &ControllerData,
    clip_names: &BTreeMap<String, usize>,
    clips: &[CookedClip],
    database_names: &BTreeMap<String, usize>,
    databases: &[MotionDatabase],
    joints: usize,
    out: &mut Vec<ProgramNode>,
    resolved: &mut BTreeMap<String, usize>,
    visiting: &mut BTreeSet<String>,
) -> Result<usize, Error> {
    if let Some(index) = resolved.get(id) {
        return Ok(*index);
    }
    ensure(
        !id.is_empty() && visiting.len() < 64 && visiting.insert(id.into()),
        "animation graph cycle or depth limit exceeded",
    )?;
    let mut node = data
        .nodes
        .get(id)
        .ok_or_else(|| Error("child node does not exist".into()))?
        .clone();
    ensure(
        node.weight.is_finite()
            && (0.0..=1.0).contains(&node.weight)
            && node
                .bone_mask
                .iter()
                .all(|w| w.is_finite() && (0.0..=1.0).contains(w))
            && (node.bone_mask.is_empty() || node.bone_mask.len() == joints),
        "invalid node weight or bone mask",
    )?;
    ensure(
        node.samples
            .iter()
            .all(|s| s.x.is_finite() && s.y.is_finite()),
        "blend coordinates must be finite",
    )?;
    let param = |name: &str| -> Result<usize, Error> {
        let (index, (_, p)) = data
            .parameters
            .iter()
            .enumerate()
            .find(|(_, (n, _))| n.as_str() == name)
            .ok_or_else(|| Error("blend parameter does not exist".into()))?;
        ensure(
            p.kind == ParameterKind::Float,
            "blend parameters must be floats",
        )?;
        Ok(index)
    };
    let mut x = 0;
    let mut y = 0;
    let mut clip = 0;
    let mut matching = None;
    match node.kind {
        NodeKind::Clip => {
            ensure(
                node.samples.is_empty(),
                "clip nodes cannot have child samples",
            )?;
            clip = *clip_names
                .get(
                    &node
                        .clip
                        .as_ref()
                        .ok_or_else(|| Error("clip node has no reference".into()))?
                        .0,
                )
                .ok_or_else(|| Error("clip binding is missing".into()))?;
        }
        NodeKind::MotionMatching => {
            ensure(
                node.samples.is_empty(),
                "motion-matching nodes cannot have children",
            )?;
            let database = *database_names
                .get(
                    &node
                        .clip
                        .as_ref()
                        .ok_or_else(|| Error("motion node has no database reference".into()))?
                        .0,
                )
                .ok_or_else(|| Error("motion database binding is missing".into()))?;
            let db = &databases[database];
            let settings = node
                .matching
                .as_ref()
                .ok_or_else(|| Error("motion node search settings are missing".into()))?;
            ensure(
                settings.query.len() == db.config().dimensions()
                    && settings.half_life.is_finite()
                    && settings.half_life > 0.0,
                "motion query dimension or half-life mismatch",
            )?;
            matching = Some(ProgramMatch {
                database,
                settings: settings.clone(),
                parameters: settings
                    .query
                    .iter()
                    .map(|p| param(p))
                    .collect::<Result<_, _>>()?,
                clips: db.references.iter().map(|r| clip_names[&r.0]).collect(),
            });
        }
        NodeKind::Blend1d => {
            x = param(&node.parameter_x)?;
            ensure(
                node.samples.len() >= 2,
                "1D blends need at least two samples",
            )?;
            node.samples.sort_by(|a, b| a.x.total_cmp(&b.x));
            ensure(
                node.samples.windows(2).all(|w| w[0].x < w[1].x),
                "1D sample coordinates must be unique",
            )?;
        }
        NodeKind::Directional2d => {
            x = param(&node.parameter_x)?;
            y = param(&node.parameter_y)?;
            ensure(
                node.samples.len() >= 4
                    && node
                        .samples
                        .iter()
                        .filter(|s| s.x == 0.0 && s.y == 0.0)
                        .count()
                        == 1,
                "directional blend needs a center and at least three ring samples",
            )?;
            node.samples.sort_by(|a, b| {
                let center_a = a.x == 0.0 && a.y == 0.0;
                let center_b = b.x == 0.0 && b.y == 0.0;
                center_b
                    .cmp(&center_a)
                    .then_with(|| a.y.atan2(a.x).total_cmp(&b.y.atan2(b.x)))
            });
            for i in 1..node.samples.len() {
                let a = &node.samples[i];
                let b = &node.samples[if i + 1 == node.samples.len() {
                    1
                } else {
                    i + 1
                }];
                ensure(
                    f64::from(a.x) * f64::from(b.y) - f64::from(a.y) * f64::from(b.x) > 0.0,
                    "directional ring must enclose the origin without duplicate rays or gaps >= 180 degrees",
                )?;
            }
        }
        NodeKind::Additive | NodeKind::Layer => {
            ensure(
                node.samples.len() == 2,
                "layer/additive nodes need base and overlay samples",
            )?;
            if !node.parameter_x.is_empty() {
                x = param(&node.parameter_x)?;
            }
        }
    }
    let mut children = Vec::new();
    for sample in &node.samples {
        children.push(visit(
            &sample.node,
            data,
            clip_names,
            clips,
            database_names,
            databases,
            joints,
            out,
            resolved,
            visiting,
        )?);
    }
    let duration = if let Some(program) = &matching {
        program
            .clips
            .iter()
            .map(|i| clips[*i].duration)
            .max()
            .unwrap_or(0)
    } else if node.kind == NodeKind::Clip {
        clips[clip].duration
    } else {
        children.iter().map(|i| out[*i].duration).max().unwrap_or(0)
    };
    visiting.remove(id);
    let index = out.len();
    resolved.insert(id.into(), index);
    out.push(ProgramNode {
        data: node,
        children,
        clip,
        x,
        y,
        duration,
        matching,
    });
    Ok(index)
}
