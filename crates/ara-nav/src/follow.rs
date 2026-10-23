//! Pure per-agent path following: corner advancement, arrival, typed link
//! traversal, link gates and stall detection. No mesh queries, so it runs in
//! parallel over ECS table slices; every result depends only on this agent.
use crate::{
    LinkKind, NavAgent, NavAgentState, NavIssue, NavStatus, OffMeshLink, Traversal, distance,
};

/// Per-tick link gates shared by all agents (sorted ascending by link ID).
#[derive(Default)]
pub(crate) struct LinkGates {
    /// Links that are currently closed (doors shut, elevator away).
    pub closed: Vec<u32>,
    /// Links planning must avoid: locked, or closed and not waitable.
    pub blocked: Vec<u32>,
}
impl LinkGates {
    pub(crate) fn is_closed(&self, id: u32) -> bool {
        self.closed.binary_search(&id).is_ok()
    }
}

/// Settings that shape following, copied from `NavConfig`.
#[derive(Clone, Copy)]
pub(crate) struct Follow {
    pub dt: f32,
    pub link_wait_ticks: u32,
    pub stall_ticks: u32,
}

/// The authored link whose endpoints match an off-mesh corner pair, and whether
/// it is traversed end-to-start.
pub(crate) fn find_link(
    links: &[OffMeshLink],
    a: [f32; 3],
    b: [f32; 3],
) -> Option<(&OffMeshLink, bool)> {
    let near = |p: [f32; 3], q: [f32; 3], r: f32| distance(p, q) <= r + 0.1;
    links.iter().find_map(|l| {
        if near(l.start, a, l.radius) && near(l.end, b, l.radius) {
            Some((l, false))
        } else if l.bidirectional && near(l.end, a, l.radius) && near(l.start, b, l.radius) {
            Some((l, true))
        } else {
            None
        }
    })
}

/// Seconds to traverse `link` from `a` to `b` for an agent of `speed`.
fn traversal_time(
    link: Option<&OffMeshLink>,
    a: [f32; 3],
    b: [f32; 3],
    speed: f32,
    dt: f32,
) -> f32 {
    let length = distance(a, b);
    let speed = speed.max(0.01);
    let kind = link.map_or(LinkKind::Walk, |l| l.kind);
    let authored = link.map_or(0.0, |l| l.duration);
    let seconds = if authored > 0.0 {
        authored
    } else {
        match kind {
            LinkKind::Walk | LinkKind::Door => length / speed,
            LinkKind::Jump => (length / speed).max(0.3),
            LinkKind::Ladder => length / (speed * 0.5),
            LinkKind::Elevator => length / 2.0,
            LinkKind::Teleport => dt,
        }
    };
    seconds.max(dt)
}

/// Advances an active traversal by one tick. Returns true while it is running
/// (the agent is then excluded from avoidance for this tick).
pub(crate) fn advance_traversal(
    state: &mut NavAgentState,
    position: &mut [f32; 3],
    dt: f32,
) -> bool {
    let Some(mut t) = state.traversal else {
        return false;
    };
    t.elapsed = (t.elapsed + dt).min(t.duration);
    let previous = *position;
    *position = t.position();
    state.velocity = std::array::from_fn(|i| (position[i] - previous[i]) / dt);
    state.desired = state.velocity;
    if t.progress() >= 1.0 {
        state.traversal = None;
        state.corner += 2;
    } else {
        state.traversal = Some(t);
    }
    true
}

/// Computes the desired (pre-avoidance) velocity, advancing corners, starting
/// traversals, waiting at closed links and flagging stalls. Clears the path when
/// a replan is required; the planner refills it within its budget.
pub(crate) fn steer(
    agent: &NavAgent,
    state: &mut NavAgentState,
    position: [f32; 3],
    links: &[OffMeshLink],
    gates: &LinkGates,
    cfg: Follow,
) -> [f32; 3] {
    let max_speed = state.max_speed(agent);
    let Some(target) = state.target else {
        if state.issue != Some(NavIssue::StartOffMesh) {
            state.status = NavStatus::Idle;
        }
        return [0.0; 3];
    };
    if distance(position, target) <= agent.stopping_distance {
        state.status = NavStatus::Arrived;
        state.issue = None;
        state.stalled = 0;
        return [0.0; 3];
    }
    if state.status == NavStatus::Arrived {
        state.status = NavStatus::Moving; // Pushed away: walk back.
    }
    if state.status == NavStatus::Unreachable || state.path.is_none() {
        return [0.0; 3];
    }
    // Route legs: reaching the current portal moves on to the next one. The old
    // corridor is kept until the budgeted replan replaces it.
    if let Some(portal) = crate::orca_step::leg_goal(state)
        && horizontal(position, portal) <= (agent.radius * 2.0).max(1.0)
        && let Some(route) = state.route.as_mut()
    {
        route.next += 1;
        state.planned_filter = None;
    }
    let reach = agent
        .stopping_distance
        .max(agent.radius)
        .max(max_speed * cfg.dt * 1.5);
    let Some(path) = state.path.as_ref() else {
        return [0.0; 3];
    };
    let corners = &path.corners;
    if corners.is_empty() {
        return [0.0; 3]; // Waiting for the next portal's cell to stream in.
    }
    let mut corner = state.corner.min(corners.len().saturating_sub(1));
    while corner + 1 < corners.len() && !corners[corner].off_mesh {
        let previous = corners[corner.saturating_sub(1)].position;
        if !rounded(
            position,
            previous,
            corners[corner].position,
            corners[corner + 1].position,
            max_speed * cfg.dt,
        ) {
            break;
        }
        corner += 1;
    }
    state.corner = corner;
    let current = corners[corner];
    if current.off_mesh
        && corner + 1 < corners.len()
        && horizontal(position, current.position) <= reach
    {
        let end = corners[corner + 1].position;
        let link = find_link(links, current.position, end);
        let id = link.map_or(0, |(l, _)| l.id);
        if id != 0 && gates.is_closed(id) {
            let waited = match state.waiting {
                Some((link, ticks)) if link == id => ticks + 1,
                _ => 1,
            };
            if waited > cfg.link_wait_ticks {
                state.waiting = None;
                state.avoid_link = Some(id);
                state.issue = Some(NavIssue::LinkTimeout { link: id });
                state.path = None;
            } else {
                state.waiting = Some((id, waited));
                state.issue = Some(NavIssue::WaitingForLink { link: id });
            }
            return [0.0; 3];
        }
        state.waiting = None;
        let (kind, arc_height) =
            link.map_or((LinkKind::Walk, 0.0), |(l, _)| (l.kind, l.arc_height));
        state.traversal = Some(Traversal {
            start: current.position,
            end,
            elapsed: 0.0,
            duration: traversal_time(
                link.map(|(l, _)| l),
                current.position,
                end,
                max_speed,
                cfg.dt,
            ),
            link: id,
            kind,
            arc_height,
        });
        return [0.0; 3];
    }
    let terminal = corner + 1 == corners.len();
    let complete = path.complete;
    let leg = crate::orca_step::leg_goal(state);
    // Final aim: the goal, the current route portal, or the end of a partial corridor.
    let end = match leg {
        Some(portal) if complete => portal,
        _ if complete => target,
        _ => current.position,
    };
    let aim = if terminal { end } else { current.position };
    let mut d = [
        aim[0] - position[0],
        aim[1] - position[1],
        aim[2] - position[2],
    ];
    let to_corner = (d[0] * d[0] + d[2] * d[2]).sqrt();
    // Anticipate the turn from outside the corner (DetourCrowd's rule; see
    // ara-nav-sys/VENDOR.md for the zlib source notice).
    if !terminal && !current.off_mesh {
        let next = corners[corner + 1].position;
        let (nx, nz) = (next[0] - position[0], next[2] - position[2]);
        let len = (nx * nx + nz * nz).sqrt();
        if len > 0.001 {
            d[0] -= nx / len * to_corner * 0.5;
            d[2] -= nz / len * to_corner * 0.5;
        }
    }
    let length = (d[0] * d[0] + d[2] * d[2]).sqrt();
    if length <= 1e-4 {
        return [0.0; 3];
    }
    let mut speed = max_speed;
    if terminal && leg.is_none() {
        // Slow down within two radii of the goal, never overshooting in one tick.
        let remaining = horizontal(position, target);
        let slow = (agent.radius * 2.0).max(0.25);
        speed = speed.min(speed * remaining / slow).min(remaining / cfg.dt);
    }
    let desired = [
        d[0] / length * speed,
        d[1] / length * speed,
        d[2] / length * speed,
    ];
    // Stall: wanting to move for a while without moving.
    let moving = (state.velocity[0].powi(2) + state.velocity[2].powi(2)).sqrt();
    if speed > 0.2 * max_speed && moving < 0.1 * max_speed {
        state.stalled += 1;
        if state.stalled > cfg.stall_ticks {
            state.stalled = 0;
            state.path = None;
            state.issue = Some(NavIssue::Stalled);
        }
    } else {
        state.stalled = 0;
        if matches!(
            state.issue,
            Some(NavIssue::Stalled | NavIssue::WaitingForLink { .. })
        ) {
            state.issue = None;
        }
    }
    desired
}

/// Funnel test: has the agent rounded `corner` so `next` is directly ahead?
/// The obstacle a funnel corner wraps lies on the incoming side of the line
/// through `corner` and `next`; once the agent is on (or across) that line the
/// straight move to `next` stays clear. Straight corners are skipped at once.
fn rounded(p: [f32; 3], previous: [f32; 3], corner: [f32; 3], next: [f32; 3], step: f32) -> bool {
    let (ox, oz) = (next[0] - corner[0], next[2] - corner[2]);
    let (px, pz) = (p[0] - corner[0], p[2] - corner[2]);
    let (ix, iz) = (previous[0] - corner[0], previous[2] - corner[2]);
    if horizontal(p, corner) <= step.max(0.02) {
        return true;
    }
    let out_len = (ox * ox + oz * oz).sqrt();
    let in_len = (ix * ix + iz * iz).sqrt();
    let side_previous = ox * iz - oz * ix;
    let side_agent = ox * pz - oz * px;
    if side_previous.abs() <= 1e-3 * out_len * in_len {
        // No turn (tile or area crossing): aiming further along the same line is
        // equivalent, and the crossing may sit inside a dynamic obstacle.
        return in_len > 1e-4 || px * ox + pz * oz > 0.0;
    }
    side_agent * side_previous <= 0.0
}

fn horizontal(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
