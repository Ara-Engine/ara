//! One brain tick: start, transitions (leaf first), periodic re-selection, then
//! the tasks of every active state, root first. At most one state change per tick.
use crate::ast::{Select, Type};
use crate::components::{
    ActionIntent, BrainState, Change, LookIntent, MoveIntent, TaskState, TaskStatus,
};
use crate::eval::{Env, next_random};
use crate::program::{CAction, CTrigger, Program, Task, Value};
use crate::trace::TraceEvent;
use ara_nav::NavStatus;

/// Utility re-selection switches only when the challenger beats the current
/// child by this factor (plus a small absolute margin).
pub const HYSTERESIS: f32 = 1.1;

/// Intents and trace output for one brain.
pub(crate) struct Out<'a> {
    pub moves: &'a mut MoveIntent,
    pub look: &'a mut LookIntent,
    pub action: &'a mut ActionIntent,
    pub trace: &'a mut Vec<TraceEvent>,
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Advances one brain by a fixed tick.
pub(crate) fn think(
    env: &mut Env<'_, '_>,
    s: &mut BrainState,
    overrides: &[(u16, Value)],
    out: &mut Out<'_>,
) {
    let p = env.program;
    s.age += 1;
    if s.hash != p.hash || s.active.is_empty() {
        start(env, s, overrides, out);
        if s.active.is_empty() {
            return; // No top-level state qualifies yet; retry next tick.
        }
    }
    let dt = env.dt;
    let mut pending: Option<(u16, &'static str)> = None;
    'transitions: for depth in (0..s.active.len()).rev() {
        let node = &p.states[s.active[depth] as usize];
        env.time = s.ticks[depth] as f32 * dt;
        let range = task_range(p, &s.active, depth);
        for (trigger, target) in &node.transitions {
            let (fire, cause) = match trigger {
                CTrigger::Done => (done(p, &s.active, depth, &s.tasks[range.clone()]), "done"),
                CTrigger::Failed => (
                    s.tasks[range.clone()]
                        .iter()
                        .any(|t| t.status == TaskStatus::Failed),
                    "failed",
                ),
                CTrigger::When(e) => (env.eval(*e, &s.vars).truthy(), "when"),
            };
            if fire {
                pending = Some((*target, cause));
                break 'transitions;
            }
        }
    }
    if pending.is_none() {
        for depth in 0..s.active.len() {
            let node = &p.states[s.active[depth] as usize];
            let every = match node.select {
                Select::First { every } | Select::Utility { every } => every,
                Select::Random => 0.0,
            };
            if every <= 0.0 || node.children.is_empty() {
                continue;
            }
            s.reselect[depth] += 1;
            if (s.reselect[depth] as f32) * dt < every {
                continue;
            }
            s.reselect[depth] = 0;
            let current = s.active.get(depth + 1).copied();
            let parent = s.active[depth];
            let (best, scores) = choose(env, s, Some(parent), out);
            let switch = match (best, current) {
                (Some(b), Some(c)) if b != c => match node.select {
                    Select::Utility { .. } => {
                        let of = |x: u16| scores.iter().find(|(i, _)| *i == x).map_or(0.0, |v| v.1);
                        of(b) > of(c) * HYSTERESIS + 0.01
                    }
                    _ => true,
                },
                (Some(_), None) => true,
                _ => false,
            };
            if let (true, Some(b)) = (switch, best) {
                pending = Some((b, "select"));
                break;
            }
        }
    }
    if let Some((target, cause)) = pending {
        change(env, s, target, cause, out);
    }
    run_tasks(env, s, out);
    for t in &mut s.ticks {
        *t = t.saturating_add(1);
    }
}

/// Tasks of active state `depth` live at this range of `BrainState::tasks`.
fn task_range(p: &Program, active: &[u16], depth: usize) -> std::ops::Range<usize> {
    let start: usize = active[..depth]
        .iter()
        .map(|s| p.states[*s as usize].tasks.len())
        .sum();
    start..start + p.states[active[depth] as usize].tasks.len()
}

/// `on done`: the state has finishing tasks and all of them succeeded.
fn done(p: &Program, active: &[u16], depth: usize, tasks: &[TaskState]) -> bool {
    let node = &p.states[active[depth] as usize];
    let mut any = false;
    for (task, state) in node.tasks.iter().zip(tasks) {
        if task.finishes() {
            any = true;
            if state.status != TaskStatus::Succeeded {
                return false;
            }
        }
    }
    any
}

fn start(env: &mut Env<'_, '_>, s: &mut BrainState, overrides: &[(u16, Value)], out: &mut Out<'_>) {
    let p = env.program;
    let previous = s.active.last().copied();
    s.hash = p.hash.clone();
    s.active.clear();
    s.ticks.clear();
    s.reselect.clear();
    s.tasks.clear();
    env.time = 0.0;
    s.vars = Vec::with_capacity(p.slots.len());
    for slot in &p.slots {
        let v = match slot.default {
            Some(e) => env.eval(e, &s.vars),
            None => Value::empty(slot.ty),
        };
        s.vars.push(v);
    }
    for (slot, value) in overrides {
        if let Some(v) = s.vars.get_mut(*slot as usize) {
            *v = value.clone();
        }
    }
    let (root, _) = choose(env, s, None, out);
    if let Some(root) = root {
        enter(env, s, root, out);
        descend(env, s, out);
        let to = *s.active.last().unwrap_or(&root);
        record(s, p, previous, to, "start", out);
    }
}

/// Picks a child of `parent` (or a top-level state) by the parent's mode.
/// Returns the choice and, for utility, every qualifying child's score.
fn choose(
    env: &mut Env<'_, '_>,
    s: &mut BrainState,
    parent: Option<u16>,
    out: &mut Out<'_>,
) -> (Option<u16>, Vec<(u16, f32)>) {
    let p = env.program;
    let (candidates, mode): (&[u16], Select) = match parent {
        Some(i) => (&p.states[i as usize].children, p.states[i as usize].select),
        None => (&p.roots, Select::First { every: 0.0 }),
    };
    env.time = 0.0;
    let mut qualifying = Vec::new();
    for &c in candidates {
        let ok = p.states[c as usize]
            .when
            .is_none_or(|w| env.eval(w, &s.vars).truthy());
        if ok {
            qualifying.push(c);
        }
    }
    match mode {
        Select::First { .. } => (qualifying.first().copied(), Vec::new()),
        Select::Random => {
            if qualifying.is_empty() {
                return (None, Vec::new());
            }
            let k = (next_random(env.rng) * qualifying.len() as f32) as usize;
            (Some(qualifying[k.min(qualifying.len() - 1)]), Vec::new())
        }
        Select::Utility { .. } => {
            let mut scores = Vec::new();
            for &c in &qualifying {
                let mut score = 1.0f32;
                for &e in &p.states[c as usize].scores {
                    score *= env.eval(e, &s.vars).float().clamp(0.0, 1.0);
                }
                scores.push((c, score));
            }
            let best = scores
                .iter()
                .fold(None::<(u16, f32)>, |best, &(c, v)| match best {
                    Some((_, b)) if b >= v => best,
                    _ => Some((c, v)),
                })
                .map(|(c, _)| c);
            s.scores.clone_from(&scores);
            if let Some(parent) = parent {
                out.trace.push(TraceEvent::Scores {
                    brain: s.id,
                    state: p.states[parent as usize].name.clone(),
                    scores: scores
                        .iter()
                        .map(|(c, v)| (p.states[*c as usize].name.clone(), *v))
                        .collect(),
                });
            }
            (best, scores)
        }
    }
}

fn enter(env: &mut Env<'_, '_>, s: &mut BrainState, state: u16, out: &mut Out<'_>) {
    let p = env.program;
    s.active.push(state);
    s.ticks.push(0);
    s.reselect.push(0);
    let node = &p.states[state as usize];
    s.tasks
        .extend(std::iter::repeat_n(TaskState::default(), node.tasks.len()));
    env.time = 0.0;
    actions(env, s, &node.enter, out);
}

fn exit_leaf(env: &mut Env<'_, '_>, s: &mut BrainState, out: &mut Out<'_>) {
    let p = env.program;
    let Some(&leaf) = s.active.last() else {
        return;
    };
    let depth = s.active.len() - 1;
    env.time = s.ticks[depth] as f32 * env.dt;
    actions(env, s, &p.states[leaf as usize].exit, out);
    let count = p.states[leaf as usize].tasks.len();
    s.tasks.truncate(s.tasks.len() - count);
    s.active.pop();
    s.ticks.pop();
    s.reselect.pop();
}

fn descend(env: &mut Env<'_, '_>, s: &mut BrainState, out: &mut Out<'_>) {
    while let Some(&leaf) = s.active.last() {
        if env.program.states[leaf as usize].children.is_empty() {
            return;
        }
        match choose(env, s, Some(leaf), out).0 {
            Some(child) => enter(env, s, child, out),
            None => return, // No child qualifies: the parent stays the leaf.
        }
    }
}

fn change(
    env: &mut Env<'_, '_>,
    s: &mut BrainState,
    target: u16,
    cause: &'static str,
    out: &mut Out<'_>,
) {
    let p = env.program;
    let from = s.active.last().copied();
    let path = p.path(target);
    let mut common = 0;
    while common < s.active.len() && common < path.len() && s.active[common] == path[common] {
        common += 1;
    }
    if common == path.len() {
        common -= 1; // Target already active: exit and re-enter it.
    }
    while s.active.len() > common {
        exit_leaf(env, s, out);
    }
    for &state in &path[common..] {
        enter(env, s, state, out);
    }
    descend(env, s, out);
    let to = *s.active.last().unwrap_or(&target);
    record(s, p, from, to, cause, out);
}

fn record(
    s: &mut BrainState,
    p: &Program,
    from: Option<u16>,
    to: u16,
    cause: &str,
    out: &mut Out<'_>,
) {
    s.last = Some(Change {
        from,
        to,
        cause: cause.to_owned(),
        at: s.age,
    });
    out.trace.push(TraceEvent::Transition {
        brain: s.id,
        from: from.map(|f| p.states[f as usize].name.clone()),
        to: p.states[to as usize].name.clone(),
        cause: cause.to_owned(),
    });
}

fn actions(env: &mut Env<'_, '_>, s: &mut BrainState, list: &[CAction], out: &mut Out<'_>) {
    for a in list {
        match a {
            CAction::Set(slot, e) => {
                let v = env.eval(*e, &s.vars);
                s.vars[*slot as usize] = v;
            }
            CAction::Clear(slot) => {
                let ty = env.program.slots[*slot as usize].ty;
                s.vars[*slot as usize] = Value::empty(ty);
            }
            CAction::Emit(name) => {
                let count = out.action.event.as_ref().map_or(0, |e| e.1) + 1;
                out.action.event = Some((name.clone(), count));
                out.trace.push(TraceEvent::Emit {
                    brain: s.id,
                    event: name.clone(),
                });
            }
        }
    }
}

fn run_tasks(env: &mut Env<'_, '_>, s: &mut BrainState, out: &mut Out<'_>) {
    let p = env.program;
    // Intents are rewritten every tick: states without movement tasks stop.
    *out.moves = MoveIntent::default();
    out.look.target = None;
    let mut index = 0;
    for depth in 0..s.active.len() {
        let node = &p.states[s.active[depth] as usize];
        env.time = s.ticks[depth] as f32 * env.dt;
        for task in &node.tasks {
            let before = s.tasks[index].status;
            run_task(env, s, index, task, out);
            let after = s.tasks[index].status;
            if before != after {
                out.trace.push(TraceEvent::Task {
                    brain: s.id,
                    state: node.name.clone(),
                    task: task.name().to_owned(),
                    status: after,
                });
            }
            s.tasks[index].ticks = s.tasks[index].ticks.saturating_add(1);
            index += 1;
        }
    }
}

fn run_task(
    env: &mut Env<'_, '_>,
    s: &mut BrainState,
    index: usize,
    task: &Task,
    out: &mut Out<'_>,
) {
    let dt = env.dt;
    if s.tasks[index].status != TaskStatus::Running {
        return;
    }
    let speed = |env: &mut Env<'_, '_>, s: &BrainState, e: Option<u32>| {
        e.map(|e| env.eval(e, &s.vars).float())
    };
    match task {
        Task::MoveTo {
            target,
            speed: v,
            tolerance,
        } => {
            let goal = env.eval(*target, &s.vars);
            let Some(goal) = env.position_of(&goal) else {
                s.tasks[index].status = TaskStatus::Failed;
                return;
            };
            let tolerance = tolerance
                .map_or(0.5, |t| env.eval(t, &s.vars).float())
                .max(0.1);
            if dist(env.position, goal) <= tolerance {
                s.tasks[index].status = TaskStatus::Succeeded;
                return;
            }
            out.moves.target = Some(goal);
            out.moves.speed = speed(env, s, *v);
            s.tasks[index].point = Some(goal);
            let unreachable = env.nav.is_some_and(|n| {
                n.status == NavStatus::Unreachable && n.target.is_some_and(|t| dist(t, goal) < 0.5)
            });
            if unreachable && s.tasks[index].ticks > 2 {
                s.tasks[index].status = TaskStatus::Failed;
            }
        }
        Task::Patrol { route, speed: v } => {
            let Value::Points(points) = env.eval(*route, &s.vars) else {
                s.tasks[index].status = TaskStatus::Failed;
                return;
            };
            if points.is_empty() {
                s.tasks[index].status = TaskStatus::Failed;
                return;
            }
            let mut k = s.tasks[index].index as usize % points.len();
            if s.tasks[index].ticks == 0 {
                // Resume from the nearest waypoint (returning after an interruption).
                k = (0..points.len())
                    .min_by(|a, b| {
                        dist(env.position, points[*a]).total_cmp(&dist(env.position, points[*b]))
                    })
                    .unwrap_or(0);
            }
            if dist(env.position, points[k]) <= 0.75 {
                k = (k + 1) % points.len();
            }
            s.tasks[index].index = k as u32;
            out.moves.target = Some(points[k]);
            out.moves.speed = speed(env, s, *v);
        }
        Task::Wait { seconds } => {
            let seconds = env.eval(*seconds, &s.vars).float();
            if s.tasks[index].ticks as f32 * dt >= seconds {
                s.tasks[index].status = TaskStatus::Succeeded;
            }
        }
        Task::Face { target } => {
            let v = env.eval(*target, &s.vars);
            out.look.target = env.position_of(&v);
        }
        Task::Act {
            name,
            target,
            every,
            duration,
        } => {
            let ticks = s.tasks[index].ticks;
            let fire = match every {
                Some(e) => {
                    let period = (env.eval(*e, &s.vars).float() / dt).round().max(1.0) as u32;
                    ticks.is_multiple_of(period)
                }
                None => ticks == 0,
            };
            if fire {
                let t = target.map(|t| env.eval(t, &s.vars));
                out.action.name.clone_from(name);
                out.action.target = match t {
                    Some(Value::Entity(id)) => Some(id),
                    _ => None,
                };
                out.action.sequence = out.action.sequence.wrapping_add(1);
                s.tasks[index].index += 1;
                out.trace.push(TraceEvent::Act {
                    brain: s.id,
                    name: name.clone(),
                    target: out.action.target,
                });
            }
            if every.is_none() {
                let d = duration.map_or(0.0, |d| env.eval(d, &s.vars).float());
                if (ticks + 1) as f32 * dt >= d {
                    s.tasks[index].status = TaskStatus::Succeeded;
                }
            }
        }
        Task::Find { query, into } => {
            let q = &env.program.queries[*query as usize];
            let (best, items) = env.run_query(q, &s.vars);
            out.trace.push(TraceEvent::Eqs {
                brain: s.id,
                query: q.name.clone(),
                best,
                items,
            });
            match best {
                Some(point) if env.program.slots[*into as usize].ty == Type::Vec3 => {
                    s.vars[*into as usize] = Value::Vec3(point);
                    s.tasks[index].status = TaskStatus::Succeeded;
                }
                _ => s.tasks[index].status = TaskStatus::Failed,
            }
        }
        Task::Wander {
            radius,
            center,
            speed: v,
        } => {
            if s.tasks[index].origin.is_none() {
                s.tasks[index].origin = Some(env.position);
            }
            let reached = s.tasks[index]
                .point
                .is_none_or(|p| dist(env.position, p) <= 1.0);
            if reached {
                let c = match center {
                    Some(c) => {
                        let v = env.eval(*c, &s.vars);
                        env.position_of(&v)
                    }
                    None => s.tasks[index].origin,
                }
                .unwrap_or(env.position);
                let r = env.eval(*radius, &s.vars).float().max(0.0);
                s.tasks[index].point = None;
                for _ in 0..8 {
                    let angle = next_random(env.rng) * std::f32::consts::TAU;
                    let d = next_random(env.rng).sqrt() * r;
                    let candidate = [c[0] + angle.cos() * d, c[1], c[2] + angle.sin() * d];
                    if let Some(p) = env.project(candidate) {
                        s.tasks[index].point = Some(p);
                        break;
                    }
                }
            }
            out.moves.target = s.tasks[index].point;
            out.moves.speed = speed(env, s, *v);
        }
    }
}
