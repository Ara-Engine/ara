//! `ara why` for NPCs. The running simulation periodically reports troubled
//! brains on the diagnostics bus (stable codes, entity subject, JSON evidence);
//! [`NpcExplainer`] combines those reports with authored components into ranked
//! causes: no path, blocked by a door/obstacle/crowd, state tree stuck in a
//! state, perception never saw the target, missing brain/agent/navmesh.
use crate::components::{BrainState, TaskStatus};
use crate::program::{CTrigger, Node, Program};
use crate::{AiWorld, codes};
use ara_diag::store::Filter;
use ara_diag::{Code, Diagnostic, FixAction, Subject};
use ara_nav::{NavAgentState, NavIssue};
use ara_perception::{Perception, SightOutcome};
use ara_sim::Identity;
use ara_why::{Cause, Collector, Explainer, Facts, Findings, Likelihood, Question, Report, Status};
use bevy_ecs::prelude::World;
use serde_json::{Value as Json, json};
use std::{collections::BTreeMap, sync::Arc};

/// Whether a state waits on perception (its transitions or `when` read senses).
fn perceptive(p: &Program, state: u16) -> bool {
    let node = &p.states[state as usize];
    let reads = |e: u32| uses_senses(p, e);
    node.transitions
        .iter()
        .any(|(t, _)| matches!(t, CTrigger::When(e) if reads(*e)))
        || node
            .children
            .iter()
            .any(|c| p.states[*c as usize].when.is_some_and(reads))
}

fn uses_senses(p: &Program, e: u32) -> bool {
    use crate::program::Builtin::*;
    match &p.exprs[e as usize] {
        Node::Call(b, args) => {
            matches!(b, Sees | Heard | Noise | Nearest | Known | Lost | LastSeen)
                || args.iter().any(|a| uses_senses(p, *a))
        }
        Node::Not(x) | Node::Neg(x) => uses_senses(p, *x),
        Node::Bin(_, a, b) => uses_senses(p, *a) || uses_senses(p, *b),
        Node::Vec3(parts) => parts.iter().any(|x| uses_senses(p, *x)),
        Node::List(items) => items.iter().any(|x| uses_senses(p, *x)),
        _ => false,
    }
}

/// System: every `report_seconds` (by each brain's own age, so deterministic and
/// snapshot-safe) emits one diagnostic per troubled brain. Free when no bus
/// consumer is attached.
pub fn report(world: &mut World) {
    let Some(ai) = world.get_resource::<AiWorld>() else {
        return;
    };
    let dt = ai.config.fixed_dt;
    let period = (ai.config.report_seconds / dt).round().max(1.0) as u64;
    let stuck = (ai.config.stuck_seconds / dt).round().max(1.0) as u32;
    let wanted = [
        codes::NO_PATH,
        codes::BLOCKED,
        codes::STUCK,
        codes::UNSEEN,
        codes::NO_BEHAVIOR,
    ];
    if !wanted.iter().any(|c| ara_diag::bus::admit(*c)) {
        return;
    }
    let programs: BTreeMap<String, Arc<Program>> = ai
        .programs_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut q = world.query::<(
        &BrainState,
        Option<&NavAgentState>,
        Option<&Perception>,
        Option<&Identity>,
    )>();
    let mut out: Vec<(u64, Diagnostic)> = Vec::new();
    for (brain, nav, perception, identity) in q.iter(world) {
        if brain.age % period != 0 {
            continue;
        }
        let subject = identity.map(|i| Subject::entity(i.0.clone()));
        let Some(program) = programs.get(&brain.program) else {
            out.push((
                brain.id,
                Diagnostic::error(
                    codes::NO_BEHAVIOR,
                    format!("brain runs unknown behaviour `{}`", brain.program),
                )
                .data(json!({"id": brain.id, "program": brain.program})),
            ));
            continue;
        };
        if let Some(d) = diagnose(program, brain, nav, perception, stuck, dt) {
            let d = match &subject {
                Some(s) => d.subject(s.clone()),
                None => d,
            };
            out.push((brain.id, d));
        }
    }
    out.sort_by_key(|(id, _)| *id);
    for (_, d) in out {
        d.emit();
    }
}

/// The most specific problem of one brain, if any.
pub fn diagnose(
    program: &Program,
    brain: &BrainState,
    nav: Option<&NavAgentState>,
    perception: Option<&Perception>,
    stuck: u32,
    dt: f32,
) -> Option<Diagnostic> {
    let leaf = *brain.active.last()?;
    let node = &program.states[leaf as usize];
    let seconds = brain.ticks.last().copied().unwrap_or(0) as f32 * dt;
    let path: Vec<&str> = brain
        .active
        .iter()
        .map(|s| program.states[*s as usize].name.as_str())
        .collect();
    let mut tasks = Vec::new();
    let mut index = 0;
    for &s in &brain.active {
        for t in &program.states[s as usize].tasks {
            let status = brain
                .tasks
                .get(index)
                .map_or(TaskStatus::Running, |t| t.status);
            tasks.push(json!({"state": program.states[s as usize].name, "task": t.name(), "status": format!("{status:?}")}));
            index += 1;
        }
    }
    let checks: Vec<Json> = perception.map_or_else(Vec::new, |p| {
        p.checks
            .iter()
            .map(|(id, o)| json!({"id": id, "outcome": format!("{o:?}")}))
            .collect()
    });
    let evidence = json!({
        "id": brain.id,
        "behavior": program.name,
        "path": path,
        "state": node.name,
        "seconds": seconds,
        "tasks": tasks,
        "transitions": node.transition_text,
        "issue": nav.and_then(|n| n.issue).map(|i| serde_json::to_value(i).unwrap_or(Json::Null)),
        "navStatus": nav.map(|n| format!("{:?}", n.status)),
        "checks": checks,
    });
    let name = &node.name;
    if let Some(issue) = nav.and_then(|n| n.issue) {
        let (code, message) = match issue {
            NavIssue::NoPath { reachable } => (
                codes::NO_PATH,
                format!(
                    "cannot reach its move target from state `{name}`; nearest reachable point {reachable:?}"
                ),
            ),
            NavIssue::StartOffMesh => (
                codes::NO_PATH,
                format!("is off the navmesh in state `{name}`"),
            ),
            NavIssue::NoMesh => (
                codes::NO_PATH,
                "has no navmesh for its agent profile".to_owned(),
            ),
            NavIssue::InvalidAgent => (
                codes::NO_PATH,
                "has agent dimensions larger than its navmesh's bake clearance".to_owned(),
            ),
            NavIssue::WaitingForLink { link } => (
                codes::BLOCKED,
                format!("is waiting at closed link {link} in state `{name}`"),
            ),
            NavIssue::LinkTimeout { link } => (
                codes::BLOCKED,
                format!("gave up waiting at link {link} and is routing around it"),
            ),
            NavIssue::Stalled => (
                codes::BLOCKED,
                format!("is blocked by an obstacle or crowd in state `{name}`"),
            ),
            NavIssue::OutsideResident => return None, // Normal long-route following.
        };
        return Some(Diagnostic::warning(code, message).data(evidence));
    }
    let ticks = brain.ticks.last().copied().unwrap_or(0);
    if ticks < stuck {
        return None;
    }
    let range = {
        let start: usize = brain.active[..brain.active.len() - 1]
            .iter()
            .map(|s| program.states[*s as usize].tasks.len())
            .sum();
        start..start + node.tasks.len()
    };
    let own = brain.tasks.get(range).unwrap_or(&[]);
    let finished = node.tasks.iter().any(|t| t.finishes())
        && node
            .tasks
            .iter()
            .zip(own)
            .all(|(t, s)| !t.finishes() || s.status != TaskStatus::Running);
    let failed = own.iter().any(|s| s.status == TaskStatus::Failed);
    if failed || finished {
        let what = if failed {
            "a task failed"
        } else {
            "its tasks finished"
        };
        return Some(
            Diagnostic::warning(
                codes::STUCK,
                format!("state tree is stuck in `{name}` for {seconds:.0} s: {what} but no transition fired"),
            )
            .data(evidence),
        );
    }
    if perceptive(program, leaf)
        && perception.is_some_and(|p| !p.checks.iter().any(|(_, o)| *o == SightOutcome::Seen))
    {
        return Some(
            Diagnostic::info(
                codes::UNSEEN,
                format!("has waited {seconds:.0} s in `{name}` for a perception trigger that never fired"),
            )
            .data(evidence),
        );
    }
    None
}

/// Explains "why isn't this NPC moving / reacting" for one entity.
pub struct NpcExplainer;

const WORDS: &[&str] = &[
    "moving",
    "move",
    "moves",
    "stuck",
    "idle",
    "npc",
    "ai",
    "patrol",
    "patrolling",
    "chase",
    "chasing",
    "attack",
    "attacking",
    "react",
    "reacting",
    "see",
    "seeing",
    "notice",
    "walk",
    "walking",
    "standing",
    "behavior",
    "behaviour",
    "brain",
    "doing",
    "path",
    "reach",
];

fn component<'a>(e: &'a ara_why::Entity, name: &str) -> Option<&'a Json> {
    e.components.get(name)
}

impl Explainer for NpcExplainer {
    fn name(&self) -> &'static str {
        "npc"
    }
    fn about(&self) -> &'static str {
        "Why an AI agent is not moving or reacting: navigation, state tree, perception and setup."
    }
    fn examples(&self) -> &'static [&'static str] {
        &["why isn't name:Guard moving", "name:Guard stuck in patrol"]
    }
    fn matches(&self, q: &Question) -> u32 {
        let topical = q.words.iter().any(|w| WORDS.contains(&w.as_str()));
        if topical && (!q.entities.is_empty() || !q.names.is_empty()) {
            60
        } else {
            0
        }
    }
    fn explain(
        &self,
        q: &Question,
        facts: &dyn Facts,
        _extra: &[&dyn Collector],
    ) -> Result<Report, Vec<Diagnostic>> {
        let selector = q
            .entities
            .first()
            .cloned()
            .or_else(|| q.names.first().map(|n| format!("name:{n}")))
            .ok_or_else(|| {
                vec![Diagnostic::error(
                    codes::NAME,
                    "name the NPC, e.g. `name:Guard`",
                )]
            })?;
        let e = facts.entity(&selector, q.doc.as_deref())?;
        let mut f = Findings::default();
        let me = Subject::entity(e.address.clone());
        let add = |component: &str| FixAction::Command {
            command: "component.add".into(),
            params: json!({"entity": e.address, "component": component}),
        };
        match component(&e, "Brain") {
            Some(brain) => {
                let reference = brain
                    .get("behavior")
                    .and_then(Json::as_str)
                    .unwrap_or_default();
                if reference.is_empty() {
                    f.check("brain", Status::Fail, "no behaviour assigned");
                    f.cause(
                        Cause::new(
                            codes::NO_BEHAVIOR,
                            Likelihood::High,
                            format!("{} has a Brain without a behaviour", e.label()),
                        )
                        .subject(me.clone())
                        .advice("Set Brain.behavior to a .behavior graph asset"),
                    );
                } else {
                    let asset = facts.asset(reference);
                    if asset.resolves() {
                        f.check("brain", Status::Ok, asset.canonical());
                    } else {
                        f.check(
                            "brain",
                            Status::Fail,
                            format!("`{reference}` does not resolve"),
                        );
                        let mut c = Cause::new(
                            codes::NO_BEHAVIOR,
                            Likelihood::High,
                            format!("behaviour `{reference}` does not resolve"),
                        )
                        .subject(me.clone());
                        for closest in &asset.closest {
                            c = c.fix(ara_diag::Fix {
                                title: format!("Use {closest}"),
                                action: FixAction::Command {
                                    command: "property.set".into(),
                                    params: json!({"entity": e.address, "path": "Brain.behavior", "value": closest}),
                                },
                            });
                        }
                        f.cause(c);
                    }
                }
            }
            None => {
                f.check("brain", Status::Fail, "no Brain component");
                f.cause(
                    Cause::new(
                        codes::NO_BEHAVIOR,
                        Likelihood::High,
                        format!("{} has no Brain", e.label()),
                    )
                    .subject(me.clone())
                    .fix(ara_diag::Fix {
                        title: "Add a Brain".into(),
                        action: add("Brain"),
                    }),
                );
            }
        }
        match component(&e, "NavAgent") {
            Some(agent) => {
                let speed = agent.get("maxSpeed").and_then(Json::as_f64).unwrap_or(3.0);
                if speed <= 0.0 {
                    f.check("nav agent", Status::Fail, "maxSpeed is 0");
                    f.cause(
                        Cause::new(codes::NO_PATH, Likelihood::High, "NavAgent.maxSpeed is 0")
                            .subject(me.clone())
                            .fix(ara_diag::Fix {
                                title: "Set maxSpeed to 3 m/s".into(),
                                action: FixAction::Command {
                                    command: "property.set".into(),
                                    params: json!({"entity": e.address, "path": "NavAgent.maxSpeed", "value": 3.0}),
                                },
                            }),
                    );
                } else {
                    f.check("nav agent", Status::Ok, format!("{speed} m/s"));
                }
            }
            None => {
                f.check("nav agent", Status::Fail, "no NavAgent component");
                f.cause(
                    Cause::new(
                        codes::NO_PATH,
                        Likelihood::Medium,
                        format!("{} has no NavAgent, so it cannot walk", e.label()),
                    )
                    .subject(me.clone())
                    .fix(ara_diag::Fix {
                        title: "Add a NavAgent".into(),
                        action: add("NavAgent"),
                    }),
                );
            }
        }
        if facts.with_component(&e.doc, "NavMeshSurface").is_empty() {
            f.check(
                "navmesh",
                Status::Fail,
                format!("no NavMeshSurface in {}", e.doc),
            );
            f.cause(
                Cause::new(
                    codes::NO_PATH,
                    Likelihood::High,
                    format!("{} has no NavMeshSurface to bake", e.doc),
                )
                .advice("Add an entity with NavMeshSurface covering the playable area"),
            );
        } else {
            f.check("navmesh", Status::Ok, "surface authored");
        }
        let records = facts.diagnostics(&Filter {
            codes: vec![
                codes::NO_PATH,
                codes::BLOCKED,
                codes::STUCK,
                codes::UNSEEN,
                codes::NO_BEHAVIOR,
            ],
            subjects: vec![me.clone()],
            ..Filter::default()
        });
        match records.last() {
            None => f.check(
                "runtime",
                Status::Unknown,
                "no AI reports yet (is Play running with the diagnostics bus attached?)",
            ),
            Some(latest) => {
                f.check("runtime", Status::Fail, latest.diagnostic.message.clone());
                let mut seen: Vec<Code> = Vec::new();
                for r in records.iter().rev() {
                    let d = &r.diagnostic;
                    if seen.contains(&d.code) {
                        continue;
                    }
                    seen.push(d.code);
                    let likelihood = if d.code == codes::UNSEEN {
                        Likelihood::Medium
                    } else {
                        Likelihood::High
                    };
                    let mut c =
                        Cause::new(d.code, likelihood, format!("{} {}", e.label(), d.message))
                            .subject(me.clone());
                    let empty = Json::Null;
                    let data = d.data.as_deref().unwrap_or(&empty);
                    if let Some(path) = data.get("path") {
                        c = c.evidence(format!("active states: {path}"));
                    }
                    if let Some(t) = data
                        .get("transitions")
                        .and_then(Json::as_array)
                        .filter(|t| !t.is_empty())
                    {
                        c = c.evidence(format!(
                            "waiting on: {}",
                            t.iter()
                                .filter_map(Json::as_str)
                                .collect::<Vec<_>>()
                                .join("; ")
                        ));
                    }
                    if let Some(tasks) = data.get("tasks") {
                        c = c.evidence(format!("tasks: {tasks}"));
                    }
                    if let Some(issue) = data.get("issue").filter(|i| !i.is_null()) {
                        c = c.evidence(format!("navigation: {issue}"));
                    }
                    if let Some(checks) = data
                        .get("checks")
                        .and_then(Json::as_array)
                        .filter(|c| !c.is_empty())
                    {
                        c = c.evidence(format!("sight checks: {}", Json::Array(checks.clone())));
                    }
                    c = match d.code {
                        x if x == codes::NO_PATH => c.advice("Run `ara why` on the target point, or check NavMeshSurface bounds, area filters and links"),
                        x if x == codes::BLOCKED => c.advice("Open the door (NavLinkState.open), move the obstacle, or reduce crowding"),
                        x if x == codes::STUCK => c.advice("Add `on done -> <State>` or `on failed -> <State>` to the stuck state"),
                        x if x == codes::UNSEEN => c.advice("Check Perceiver range/FOV, Stimulus visibility and line of sight to the target"),
                        _ => c,
                    };
                    f.cause(c);
                }
            }
        }
        let answer = Report::verdict(
            &f,
            "brain, agent and navmesh look fine and the game reported no AI problems",
        );
        Ok(Report::new(
            &q.text,
            "npc",
            Some(format!("{} in {}", e.label(), e.doc)),
            answer,
            f,
        ))
    }
}
