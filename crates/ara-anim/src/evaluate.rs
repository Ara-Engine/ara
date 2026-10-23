use crate::{
    Animator, Controller, Error, RootMotion, Skeleton,
    compile::{ProgramNode, ProgramState},
    controller::{Event, Lane},
    graph::{NodeKind, ParameterValue},
};
use glam::{Quat, Vec3A, Vec4};

#[derive(Clone, Copy)]
struct Weights {
    indices: [usize; 3],
    values: [f32; 3],
    len: usize,
}
fn weights(node: &ProgramNode, parameters: &[ParameterValue]) -> Weights {
    let mut out = Weights {
        indices: [0; 3],
        values: [0.0; 3],
        len: 0,
    };
    match node.data.kind {
        NodeKind::Blend1d => {
            let x = f64::from(parameters[node.x].float());
            let right = node
                .data
                .samples
                .partition_point(|s| f64::from(s.x) <= x)
                .min(node.children.len() - 1);
            let left = right.saturating_sub(1);
            let a = f64::from(node.data.samples[left].x);
            let b = f64::from(node.data.samples[right].x);
            let w = if a == b {
                0.0
            } else {
                ((x - a) / (b - a)).clamp(0.0, 1.0) as f32
            };
            out = Weights {
                indices: [node.children[left], node.children[right], 0],
                values: [1.0 - w, w, 0.0],
                len: 2,
            };
        }
        NodeKind::Directional2d => {
            let x = f64::from(parameters[node.x].float());
            let y = f64::from(parameters[node.y].float());
            for i in 1..node.children.len() {
                let j = if i + 1 == node.children.len() {
                    1
                } else {
                    i + 1
                };
                let a = &node.data.samples[i];
                let b = &node.data.samples[j];
                let (ax, ay, bx, by) = (
                    f64::from(a.x),
                    f64::from(a.y),
                    f64::from(b.x),
                    f64::from(b.y),
                );
                let determinant = ax * by - ay * bx;
                let u = (x * by - y * bx) / determinant;
                let v = (ax * y - ay * x) / determinant;
                if u >= -1e-12 && v >= -1e-12 {
                    let u = u.max(0.0);
                    let v = v.max(0.0);
                    let total = (u + v).max(1.0);
                    out = Weights {
                        indices: [node.children[0], node.children[i], node.children[j]],
                        values: [
                            (1.0 - (u + v) / total) as f32,
                            (u / total) as f32,
                            (v / total) as f32,
                        ],
                        len: 3,
                    };
                    break;
                }
            }
        }
        _ => {}
    }
    out
}
fn layer_weight(node: &ProgramNode, parameters: &[ParameterValue]) -> f32 {
    if node.data.parameter_x.is_empty() {
        node.data.weight
    } else {
        parameters[node.x].float().clamp(0.0, 1.0)
    }
}

impl Animator {
    pub(crate) fn render<'a>(
        &mut self,
        c: &'a Controller,
        skeleton: &Skeleton,
        previous: Lane,
        current: Lane,
        target: Option<(Lane, Lane)>,
        emit: &mut impl FnMut(Event<'a>),
    ) -> Result<RootMotion, Error> {
        let weight = self
            .state
            .fade
            .map_or(0.0, |f| f.elapsed as f32 / f.duration as f32);
        let mut root = self.evaluate(c, skeleton, previous, current)?;
        let node = c.states[current.state].node;
        self.output.copy_from(&self.scratch[node].pose)?;
        self.curves.copy_from_slice(&self.scratch[node].curves);
        self.events(
            c,
            previous,
            current,
            0,
            if target.is_some() { 1.0 - weight } else { 1.0 },
            emit,
        );
        if let Some((previous, current)) = target {
            let target_root = self.evaluate(c, skeleton, previous, current)?;
            let node = c.states[current.state].node;
            self.output.blend(&self.scratch[node].pose, weight, None)?;
            for (out, value) in self.curves.iter_mut().zip(&self.scratch[node].curves) {
                *out += (*value - *out) * weight;
            }
            root = root.blend(target_root, weight);
            self.events(c, previous, current, 1, weight, emit);
        }
        if c.extract {
            self.output.translations[c.root] = skeleton.bind.translations[c.root];
            self.output.rotations[c.root] = skeleton.bind.rotations[c.root];
        }
        Ok(if c.extract {
            root
        } else {
            RootMotion::default()
        })
    }
    fn evaluate(
        &mut self,
        c: &Controller,
        skeleton: &Skeleton,
        previous: Lane,
        current: Lane,
    ) -> Result<RootMotion, Error> {
        let state = &c.states[current.state];
        let local = if state.looping {
            current.time % u64::from(state.duration)
        } else {
            current.time
        };
        for &index in &state.plan {
            let node = &c.nodes[index];
            let (inputs, outputs) = self.scratch.split_at_mut(index);
            let out = &mut outputs[0];
            out.curves.fill(0.0);
            match node.data.kind {
                NodeKind::Clip => {
                    let clip = &c.clips[node.clip];
                    let tick = local as f64 * f64::from(clip.duration) / f64::from(state.duration);
                    self.samplers[index]
                        .as_mut()
                        .expect("compiled clip nodes have a sampler")
                        .sample_at(clip, skeleton, tick, &mut out.pose, &mut out.sampled_curves)?;
                    for (i, value) in out.sampled_curves.iter().enumerate() {
                        out.curves[c.curve_maps[node.clip][i]] = *value;
                    }
                    out.root = root_for(clip, skeleton, previous.time, current.time, state);
                }
                NodeKind::Blend1d | NodeKind::Directional2d => {
                    let w = weights(node, &self.state.parameters);
                    let first = (0..w.len).find(|i| w.values[*i] > 0.0).unwrap_or(0);
                    if w.len == 2 {
                        out.pose.copy_from(&inputs[w.indices[0]].pose)?;
                        out.pose
                            .blend(&inputs[w.indices[1]].pose, w.values[1], None)?;
                    } else {
                        for bone in 0..c.joints {
                            let reference = inputs[w.indices[first]].pose.rotations[bone];
                            let mut translation = Vec3A::ZERO;
                            let mut rotation = Vec4::ZERO;
                            let mut scale = Vec3A::ZERO;
                            for j in 0..w.len {
                                if w.values[j] == 0.0 {
                                    continue;
                                }
                                let p = &inputs[w.indices[j]].pose;
                                translation += p.translations[bone] * w.values[j];
                                let q = p.rotations[bone];
                                rotation += Vec4::from_array(q.to_array())
                                    * if q.dot(reference) < 0.0 {
                                        -w.values[j]
                                    } else {
                                        w.values[j]
                                    };
                                scale += p.scales[bone] * w.values[j];
                            }
                            out.pose.translations[bone] = translation;
                            out.pose.rotations[bone] =
                                Quat::from_array(rotation.normalize().to_array());
                            out.pose.scales[bone] = scale;
                        }
                    }
                    let mut translation = glam::Vec3::ZERO;
                    let mut rotation = Vec4::ZERO;
                    let reference = inputs[w.indices[first]].root.rotation;
                    for j in 0..w.len {
                        let input = &inputs[w.indices[j]];
                        translation += input.root.translation * w.values[j];
                        rotation += Vec4::from_array(input.root.rotation.to_array())
                            * if input.root.rotation.dot(reference) < 0.0 {
                                -w.values[j]
                            } else {
                                w.values[j]
                            };
                        for (out, value) in out.curves.iter_mut().zip(&input.curves) {
                            *out += *value * w.values[j];
                        }
                    }
                    out.root = RootMotion {
                        translation,
                        rotation: Quat::from_array(rotation.normalize().to_array()),
                    };
                }
                NodeKind::Additive | NodeKind::Layer => {
                    let base = &inputs[node.children[0]];
                    let overlay = &inputs[node.children[1]];
                    let w = layer_weight(node, &self.state.parameters);
                    let mask = if node.data.bone_mask.is_empty() {
                        None
                    } else {
                        Some(node.data.bone_mask.as_slice())
                    };
                    let root_weight = w * mask.map_or(1.0, |m| m[c.root]);
                    out.pose.copy_from(&base.pose)?;
                    if node.data.kind == NodeKind::Additive {
                        out.pose.add(&overlay.pose, &skeleton.bind, w, mask)?;
                        out.root = base
                            .root
                            .compose(RootMotion::default().blend(overlay.root, root_weight));
                        for (i, out) in out.curves.iter_mut().enumerate() {
                            *out = base.curves[i] + overlay.curves[i] * w;
                        }
                    } else {
                        out.pose.blend(&overlay.pose, w, mask)?;
                        out.root = base.root.blend(overlay.root, root_weight);
                        for (i, out) in out.curves.iter_mut().enumerate() {
                            *out = base.curves[i] + (overlay.curves[i] - base.curves[i]) * w;
                        }
                    }
                }
            }
        }
        Ok(self.scratch[state.node].root)
    }
    fn events<'a>(
        &mut self,
        c: &'a Controller,
        previous: Lane,
        current: Lane,
        lane: u8,
        weight: f32,
        emit: &mut impl FnMut(Event<'a>),
    ) {
        if previous.time == current.time || weight == 0.0 {
            return;
        }
        let state = &c.states[current.state];
        self.event_weights.fill(0.0);
        self.event_weights[state.node] = weight;
        for &index in state.plan.iter().rev() {
            let node = &c.nodes[index];
            let weight = self.event_weights[index];
            if weight == 0.0 {
                continue;
            }
            match node.data.kind {
                NodeKind::Clip => {
                    let clip = &c.clips[node.clip];
                    let first_cycle = if state.looping {
                        previous.time / u64::from(state.duration)
                    } else {
                        0
                    };
                    let last_cycle = if state.looping {
                        current.time / u64::from(state.duration)
                    } else {
                        0
                    };
                    for cycle in first_cycle..=last_cycle {
                        for notify in &clip.notifies {
                            let time = u128::from(cycle)
                                * u128::from(state.duration)
                                * u128::from(clip.duration)
                                + u128::from(notify.tick) * u128::from(state.duration);
                            if time > u128::from(previous.time) * u128::from(clip.duration)
                                && time <= u128::from(current.time) * u128::from(clip.duration)
                            {
                                emit(Event {
                                    tick: self.state.tick,
                                    lane,
                                    clip: node.clip,
                                    notify,
                                    cycle,
                                    weight,
                                });
                            }
                        }
                    }
                }
                NodeKind::Blend1d | NodeKind::Directional2d => {
                    let w = weights(node, &self.state.parameters);
                    for j in 0..w.len {
                        self.event_weights[w.indices[j]] += weight * w.values[j];
                    }
                }
                NodeKind::Layer | NodeKind::Additive => {
                    let w = layer_weight(node, &self.state.parameters);
                    self.event_weights[node.children[0]] += weight
                        * if node.data.kind == NodeKind::Layer {
                            1.0 - w
                        } else {
                            1.0
                        };
                    self.event_weights[node.children[1]] += weight * w;
                }
            }
        }
    }
}
fn root_for(
    clip: &crate::CookedClip,
    skeleton: &Skeleton,
    previous: u64,
    current: u64,
    state: &ProgramState,
) -> RootMotion {
    if previous == current {
        return RootMotion::default();
    }
    let d = u64::from(state.duration);
    let at = |time: u64| {
        clip.root_at(
            skeleton,
            (time % d) as f64 * f64::from(clip.duration) / f64::from(state.duration),
        )
    };
    let (start, cycle) = clip.root_cache;
    let a = RootMotion::between(start, at(previous));
    let b = RootMotion::between(start, at(current));
    a.inverse()
        .compose(cycle.pow(current / d - previous / d))
        .compose(b)
}
