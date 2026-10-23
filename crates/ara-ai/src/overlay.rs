//! Capture overlays: typed primitives (renderer-independent) for paths,
//! perception cones and sight checks, EQS points and agents, plus a top-down SVG
//! so agents and humans can look at a headless capture.
use crate::components::BrainState;
use crate::trace::TraceEvent;
use crate::{AiWorld, EYE};
use ara_nav::{DebugPrimitive, NavAgent, NavAgentState, NavWorld};
use ara_perception::{Perceiver, Perception, PerceptionPose, SightOutcome};
use bevy_ecs::prelude::World;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// One overlay primitive (world space, Y-up metres).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Overlay {
    /// Navmesh triangle, link or path segment.
    Nav {
        /// Navigation debug primitive.
        primitive: DebugPrimitive,
    },
    /// An agent's remaining corridor.
    Path {
        /// Agent ID.
        id: u64,
        /// Corner positions from the agent onward.
        points: Vec<[f32; 3]>,
    },
    /// A perceiver's sight cone.
    Cone {
        /// Perceiver ID.
        id: u64,
        /// Eye position.
        eye: [f32; 3],
        /// Facing direction.
        forward: [f32; 3],
        /// Sight range in metres.
        range: f32,
        /// Full field of view in degrees.
        fov: f32,
    },
    /// The latest sight check from a perceiver to a candidate.
    Sight {
        /// Eye position.
        from: [f32; 3],
        /// Target position.
        to: [f32; 3],
        /// Outcome.
        outcome: SightOutcome,
    },
    /// One EQS candidate from this tick.
    Eqs {
        /// Querying brain.
        brain: u64,
        /// Candidate point.
        point: [f32; 3],
        /// Weighted score.
        score: f32,
        /// Whether it passed every filter.
        passed: bool,
        /// Whether it was chosen.
        best: bool,
    },
    /// An agent and its current leaf state.
    Agent {
        /// Agent ID.
        id: u64,
        /// Feet position.
        position: [f32; 3],
        /// Collision radius.
        radius: f32,
        /// Leaf state name (or empty).
        label: String,
    },
}

/// Collects overlays from the live world: navmesh (profile 0), paths, cones,
/// sight checks, this tick's EQS points and agents labelled with their state.
pub fn collect(world: &mut World, navmesh: bool) -> Vec<Overlay> {
    let mut out = Vec::new();
    if navmesh && let Some(mesh) = world.get_resource::<NavWorld>().and_then(|n| n.mesh(0)) {
        mesh.debug_stream(|primitive| out.push(Overlay::Nav { primitive }));
    }
    let mut poses: Vec<(u64, [f32; 3])> = world
        .query::<&PerceptionPose>()
        .iter(world)
        .map(|p| (p.id, p.position))
        .collect();
    poses.sort_by_key(|p| p.0);
    let mut rows: Vec<_> = world
        .query::<(
            &PerceptionPose,
            Option<&Perceiver>,
            Option<&Perception>,
            Option<&NavAgentState>,
            Option<&NavAgent>,
            Option<&BrainState>,
        )>()
        .iter(world)
        .map(|(pose, sense, memory, nav, agent, brain)| {
            (
                *pose,
                sense.copied(),
                memory.cloned(),
                nav.cloned(),
                agent.copied(),
                brain.cloned(),
            )
        })
        .collect();
    rows.sort_by_key(|r| r.0.id);
    let ai = world.get_resource::<AiWorld>();
    for (pose, sense, memory, nav, agent, brain) in rows {
        let eye = [
            pose.position[0],
            pose.position[1] + sense.map_or(EYE, |s| s.eye_height),
            pose.position[2],
        ];
        if let Some(s) = sense {
            out.push(Overlay::Cone {
                id: pose.id,
                eye,
                forward: pose.forward,
                range: s.sight_range,
                fov: s.fov,
            });
        }
        if let Some(m) = &memory {
            for (id, outcome) in &m.checks {
                if let Ok(i) = poses.binary_search_by_key(id, |p| p.0) {
                    out.push(Overlay::Sight {
                        from: eye,
                        to: poses[i].1,
                        outcome: *outcome,
                    });
                }
            }
        }
        if let Some(path) = nav
            .as_ref()
            .and_then(|n| n.path.as_ref().map(|p| (n.corner, p)))
        {
            let mut points = vec![pose.position];
            points.extend(path.1.corners.iter().skip(path.0).map(|c| c.position));
            out.push(Overlay::Path {
                id: pose.id,
                points,
            });
        }
        let label = brain
            .as_ref()
            .and_then(|b| {
                let program = ai?.program(&b.program)?;
                let leaf = *b.active.last()?;
                Some(program.states.get(leaf as usize)?.name.clone())
            })
            .unwrap_or_default();
        if agent.is_some() || brain.is_some() {
            out.push(Overlay::Agent {
                id: pose.id,
                position: pose.position,
                radius: agent.map_or(0.3, |a| a.radius),
                label,
            });
        }
    }
    if let Some(ai) = world.get_resource::<AiWorld>() {
        for e in ai.trace() {
            if let TraceEvent::Eqs {
                brain, best, items, ..
            } = e
            {
                for item in items {
                    out.push(Overlay::Eqs {
                        brain: *brain,
                        point: item.point,
                        score: item.score,
                        passed: item.passed,
                        best: Some(item.point) == *best,
                    });
                }
            }
        }
    }
    out
}

/// Renders overlays top-down (X right, Z down) to a standalone SVG of `size` px.
pub fn svg(items: &[Overlay], size: u32) -> String {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let mut grow = |p: [f32; 3]| {
        min = [min[0].min(p[0]), min[1].min(p[2])];
        max = [max[0].max(p[0]), max[1].max(p[2])];
    };
    for item in items {
        match item {
            Overlay::Nav {
                primitive: DebugPrimitive::Triangle { vertices, .. },
            } => vertices.iter().for_each(|v| grow(*v)),
            Overlay::Path { points, .. } => points.iter().for_each(|p| grow(*p)),
            Overlay::Agent { position, .. } => grow(*position),
            Overlay::Eqs { point, .. } => grow(*point),
            _ => {}
        }
    }
    if !min[0].is_finite() {
        min = [0.0, 0.0];
        max = [1.0, 1.0];
    }
    let span = (max[0] - min[0]).max(max[1] - min[1]).max(1.0) * 1.05;
    let scale = size as f32 / span;
    let px = |p: [f32; 3]| ((p[0] - min[0]) * scale + 4.0, (p[2] - min[1]) * scale + 4.0);
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{0}" height="{0}" viewBox="0 0 {0} {0}" font-family="monospace" font-size="11">"#,
        size + 8
    );
    let _ = writeln!(s, r##"<rect width="100%" height="100%" fill="#111418"/>"##);
    for item in items {
        match item {
            Overlay::Nav { primitive } => match primitive {
                DebugPrimitive::Triangle { vertices, area } => {
                    let pts: Vec<String> = vertices
                        .iter()
                        .map(|v| {
                            let (x, y) = px(*v);
                            format!("{x:.1},{y:.1}")
                        })
                        .collect();
                    let fill = if *area == 0 { "#2b3440" } else { "#3d3550" };
                    let _ = writeln!(
                        s,
                        r##"<polygon points="{}" fill="{fill}" stroke="#36414f" stroke-width="0.5"/>"##,
                        pts.join(" ")
                    );
                }
                DebugPrimitive::Link {
                    start,
                    end,
                    enabled,
                    ..
                } => {
                    let ((x1, y1), (x2, y2)) = (px(*start), px(*end));
                    let color = if *enabled { "#e0b040" } else { "#806040" };
                    let _ = writeln!(
                        s,
                        r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{color}" stroke-width="2" stroke-dasharray="4 3"/>"#
                    );
                }
                _ => {}
            },
            Overlay::Cone {
                eye,
                forward,
                range,
                fov,
                ..
            } => {
                let (cx, cy) = px(*eye);
                let base = forward[2].atan2(forward[0]);
                let half = fov.to_radians().min(std::f32::consts::TAU) * 0.5;
                let r = range * scale;
                if *fov >= 360.0 {
                    let _ = writeln!(
                        s,
                        r##"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="#f0d05014" stroke="#f0d05060"/>"##
                    );
                } else {
                    let (a0, a1) = (base - half, base + half);
                    let (x0, y0) = (cx + r * a0.cos(), cy + r * a0.sin());
                    let (x1, y1) = (cx + r * a1.cos(), cy + r * a1.sin());
                    let large = u8::from(2.0 * half > std::f32::consts::PI);
                    let _ = writeln!(
                        s,
                        r##"<path d="M{cx:.1},{cy:.1} L{x0:.1},{y0:.1} A{r:.1},{r:.1} 0 {large} 1 {x1:.1},{y1:.1} Z" fill="#f0d05014" stroke="#f0d05060"/>"##
                    );
                }
            }
            Overlay::Sight { from, to, outcome } => {
                let ((x1, y1), (x2, y2)) = (px(*from), px(*to));
                let color = match outcome {
                    SightOutcome::Seen => "#50e070",
                    SightOutcome::Occluded => "#e05050",
                    SightOutcome::OutsideCone => "#a07070",
                    SightOutcome::OutOfRange => "#606060",
                };
                let _ = writeln!(
                    s,
                    r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{color}" stroke-width="1"/>"#
                );
            }
            Overlay::Path { points, .. } => {
                let pts: Vec<String> = points
                    .iter()
                    .map(|p| {
                        let (x, y) = px(*p);
                        format!("{x:.1},{y:.1}")
                    })
                    .collect();
                let _ = writeln!(
                    s,
                    r##"<polyline points="{}" fill="none" stroke="#4aa3ff" stroke-width="1.5"/>"##,
                    pts.join(" ")
                );
            }
            Overlay::Eqs {
                point,
                score,
                passed,
                best,
                ..
            } => {
                let (x, y) = px(*point);
                if *passed {
                    let alpha = (0.25 + score.clamp(0.0, 3.0) / 4.0).min(1.0);
                    let stroke = if *best {
                        r##" stroke="#ffffff" stroke-width="2""##
                    } else {
                        ""
                    };
                    let _ = writeln!(
                        s,
                        r##"<circle cx="{x:.1}" cy="{y:.1}" r="3.5" fill="#c070ff" fill-opacity="{alpha:.2}"{stroke}/>"##
                    );
                } else {
                    let _ = writeln!(
                        s,
                        r##"<circle cx="{x:.1}" cy="{y:.1}" r="2" fill="#505050"/>"##
                    );
                }
            }
            Overlay::Agent {
                position,
                radius,
                label,
                ..
            } => {
                let (x, y) = px(*position);
                let r = (radius * scale).max(3.0);
                let _ = writeln!(
                    s,
                    r##"<circle cx="{x:.1}" cy="{y:.1}" r="{r:.1}" fill="#ff8040"/>"##
                );
                if !label.is_empty() {
                    let _ = writeln!(
                        s,
                        r##"<text x="{:.1}" y="{:.1}" fill="#e8e8e8">{}</text>"##,
                        x + r + 2.0,
                        y - r,
                        xml(label)
                    );
                }
            }
        }
    }
    s.push_str("</svg>\n");
    s
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
