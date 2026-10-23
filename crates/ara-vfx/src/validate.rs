use crate::{Effect, Module, Namespace, Stacks, Stage, error};
use ara_diag::Diagnostic;
use std::collections::BTreeSet;

fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
}
fn nonzero(v: &[f32; 3]) -> bool {
    v.iter().map(|x| x * x).sum::<f32>() > 0.000001
}
fn keys(ts: impl Iterator<Item = f32>) -> bool {
    let ts = ts.collect::<Vec<_>>();
    (2..=16).contains(&ts.len())
        && ts[0] >= 0.
        && ts[ts.len() - 1] <= 1.
        && ts.windows(2).all(|p| p[0] < p[1])
}
impl Effect {
    /// Validate stage legality, finite values, bindings, geometry and resource budgets.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if self.name.trim().is_empty()
            || !(1..=240).contains(&self.hz)
            || self.emitters.is_empty()
            || self.emitters.len() > 16
            || self.quality.particles == 0
            || self.quality.particles > 1_048_576
            || !(0.01..=1024.).contains(&self.quality.overdraw)
            || self.quality.lights > 64
            || self.quality.events == 0
            || self.quality.events > 4096
        {
            return Err(error(
                "effect needs a name, 1..240 Hz, 1..16 emitters and bounded nonzero quality limits",
            ));
        }
        // serde_json serializes nonfinite floats as null; reject those before publishing.
        fn null(v: &serde_json::Value) -> bool {
            match v {
                serde_json::Value::Null => true,
                serde_json::Value::Array(a) => a.iter().any(null),
                serde_json::Value::Object(o) => o.values().any(null),
                _ => false,
            }
        }
        if null(&crate::encode(self)?) {
            return Err(error("all VFX numeric values must be finite"));
        }
        let mut parameters = BTreeSet::new();
        for p in &self.parameters {
            if !name(&p.name) || p.doc.trim().is_empty() || !parameters.insert(p.key()) {
                return Err(error("parameters need unique names and documentation"));
            }
            if p.namespace == Namespace::Emitter
                && !self
                    .emitters
                    .iter()
                    .any(|e| p.name.starts_with(&format!("{}.", e.name)))
            {
                return Err(error("emitter parameter must start with its emitter name"));
            }
        }
        let mut emitters = BTreeSet::new();
        for e in &self.emitters {
            if !name(&e.name)
                || !emitters.insert(&e.name)
                || e.capacity == 0
                || e.capacity > 1_048_576
                || !(0..3).all(|i| e.bounds.min[i] < e.bounds.max[i])
            {
                return Err(error(format!(
                    "invalid emitter {}, capacity or bounds",
                    e.name
                )));
            }
            let mut names = BTreeSet::new();
            let mut geometry = 0;
            let mut lifetimes = 0;
            for stage in Stacks::STAGES {
                if e.stacks.get(stage).len() > 64 {
                    return Err(error("at most 64 modules per stack"));
                }
                for item in e.stacks.get(stage) {
                    if !name(&item.name)
                        || !names.insert(&item.name)
                        || item.module.stage() != stage
                    {
                        return Err(error(format!(
                            "module {} has duplicate name or wrong stage",
                            item.name
                        )));
                    }
                    for key in item.bindings.values() {
                        if !parameters.contains(key)
                            || key.starts_with("emitter.")
                                && !key.starts_with(&format!("emitter.{}.", e.name))
                        {
                            return Err(error(format!("unknown or out-of-scope parameter {key}")));
                        }
                    }
                    let valid = match &item.module {
                        Module::Appearance {
                            stretch, shutter, ..
                        } => (1. ..=64.).contains(stretch) && (0. ..=1.).contains(shutter),
                        Module::Rate { per_second } => {
                            *per_second >= 0. && *per_second <= 100_000_000.
                        }
                        Module::Burst { count, .. } => *count <= 1_048_576,
                        Module::Lifetime {
                            seconds,
                            size,
                            color,
                        } => {
                            lifetimes += 1;
                            *seconds > 0.
                                && *seconds <= 86400.
                                && *size >= 0.
                                && color.iter().all(|v| *v >= 0.)
                                && color[3] <= 1.
                        }
                        Module::Sphere { radius, .. } => *radius >= 0.,
                        Module::Box { extent, .. } => extent.iter().all(|v| *v >= 0.),
                        Module::Cone { radius, height, .. } => *radius >= 0. && *height >= 0.,
                        Module::Velocity { spread, .. } => spread.iter().all(|v| *v >= 0.),
                        Module::Drag { coefficient } => *coefficient >= 0.,
                        Module::Vortex { axis, .. } => nonzero(axis),
                        Module::Attractor { softening, .. } => *softening > 0.,
                        Module::ColorOverLife { keys: ks } => {
                            keys(ks.iter().map(|k| k.t))
                                && ks
                                    .iter()
                                    .all(|k| k.value.iter().all(|v| *v >= 0.) && k.value[3] <= 1.)
                        }
                        Module::SizeOverLife { keys: ks } => {
                            keys(ks.iter().map(|k| k.t)) && ks.iter().all(|k| k.value >= 0.)
                        }
                        Module::PlaneCollision {
                            normal,
                            restitution,
                            ..
                        } => nonzero(normal) && (0. ..=1.).contains(restitution),
                        Module::DepthCollision {
                            restitution,
                            thickness,
                        } => (0. ..=1.).contains(restitution) && *thickness > 0.,
                        Module::Sprite {
                            domain,
                            soft_distance,
                            ..
                        } => {
                            geometry += 1;
                            valid_domain(*domain) && *soft_distance >= 0.
                        }
                        Module::Ribbon { width, domain } => {
                            geometry += 1;
                            *width > 0. && valid_domain(*domain)
                        }
                        Module::Mesh {
                            triangles, domain, ..
                        } => {
                            geometry += 1;
                            valid_domain(*domain) && valid_triangles(triangles)
                        }
                        Module::MeshSurface { triangles } => valid_triangles(triangles),
                        Module::Flipbook { grid, fps } => {
                            grid.iter().all(|v| *v > 0 && *v <= 256) && *fps >= 0.
                        }
                        Module::Light {
                            lumens,
                            radius,
                            color,
                        } => *lumens >= 0. && *radius > 0. && color.iter().all(|v| *v >= 0.),
                        Module::EventSpawn { target, count, .. } => {
                            self.emitters.iter().any(|e| &e.name == target)
                                && (1..=64).contains(count)
                        }
                        _ => true,
                    };
                    if !valid {
                        return Err(error(format!(
                            "invalid {} module {} in {}",
                            item.module.kind(),
                            item.name,
                            e.name
                        )));
                    }
                }
            }
            if geometry != 1 || lifetimes != 1 {
                return Err(error(format!(
                    "{} needs exactly one lifetime and one geometry module",
                    e.name
                )));
            }
            if e.stacks
                .get(Stage::Render)
                .iter()
                .filter(|m| {
                    matches!(
                        m.module,
                        Module::EventSpawn {
                            trigger: crate::Trigger::Death,
                            ..
                        }
                    )
                })
                .count()
                > 1
                || e.stacks
                    .get(Stage::Render)
                    .iter()
                    .filter(|m| {
                        matches!(
                            m.module,
                            Module::EventSpawn {
                                trigger: crate::Trigger::Collision,
                                ..
                            }
                        )
                    })
                    .count()
                    > 1
            {
                return Err(error(
                    "one destination per trigger per emitter; use several emitters for fanout",
                ));
            }
        }
        Ok(())
    }
    /// Report quality reductions before simulation; no silent allocation truncation.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut result = Vec::new();
        if self
            .emitters
            .iter()
            .map(|e| u64::from(e.capacity))
            .sum::<u64>()
            > u64::from(self.quality.particles)
        {
            result.push(Diagnostic::warning(
                crate::QUALITY,
                "requested particles exceed quality cap; emitters are allocated in authored order",
            ));
        }
        let lights = self
            .emitters
            .iter()
            .flat_map(|e| &e.stacks.render)
            .filter(|m| matches!(m.module, Module::Light { .. }))
            .count();
        if lights > self.quality.lights as usize {
            result.push(Diagnostic::warning(
                crate::QUALITY,
                "emitter lights exceed quality cap; later lights are omitted",
            ));
        }
        result
    }
}
fn valid_domain(d: ara_material::Domain) -> bool {
    matches!(
        d,
        ara_material::Domain::Surface | ara_material::Domain::Unlit | ara_material::Domain::Ui
    )
}
fn valid_triangles(ts: &[[[f32; 3]; 3]]) -> bool {
    !ts.is_empty()
        && ts.len() <= 4096
        && ts.iter().all(|t| {
            (glam::Vec3::from(t[1]) - glam::Vec3::from(t[0]))
                .cross(glam::Vec3::from(t[2]) - glam::Vec3::from(t[0]))
                .length_squared()
                > 1e-12
        })
}
