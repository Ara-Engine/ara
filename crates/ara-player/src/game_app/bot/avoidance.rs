//! The bot steers around visible hinged colliders missing from the static navmesh.
use ara_components::Transform;
use ara_physics::Collider;
use glam::Vec2;

pub(super) fn around_box(
    position: [f32; 3],
    mut goal: [f32; 3],
    destination: [f32; 3],
    pose: &Transform,
    collider: &Collider,
    clearance: f32,
) -> [f32; 3] {
    let yaw = pose.rotation[1].to_radians();
    let x = Vec2::new(yaw.cos(), -yaw.sin());
    let z = Vec2::new(yaw.sin(), yaw.cos());
    let center = Vec2::new(pose.position[0], pose.position[2])
        + x * collider.center[0]
        + z * collider.center[2];
    let local = |p: [f32; 3]| {
        let delta = Vec2::new(p[0], p[2]) - center;
        Vec2::new(delta.dot(x), delta.dot(z))
    };
    let extent =
        Vec2::new(collider.half_extents[0], collider.half_extents[2]) + Vec2::splat(clearance);
    let start = local(position);
    if start.abs().cmplt(extent).all() {
        let gap = extent - start.abs();
        let axis = usize::from(gap.y < gap.x);
        let mut exit = start;
        exit[axis] = if start[axis] < 0. {
            -extent[axis] - 0.15
        } else {
            extent[axis] + 0.15
        };
        let point = center + x * exit.x + z * exit.y;
        return [point.x, goal[1], point.y];
    }
    let mut end = local(goal);
    if end.abs().cmplt(extent).all() {
        goal = destination;
        end = local(goal);
    }
    if !blocked(start, end, extent) {
        return goal;
    }
    let corners = extent + Vec2::splat(0.02);
    let nodes = [
        start,
        end,
        Vec2::new(-corners.x, -corners.y),
        Vec2::new(corners.x, -corners.y),
        Vec2::new(corners.x, corners.y),
        Vec2::new(-corners.x, corners.y),
    ];
    let mut costs = [f32::INFINITY; 6];
    let mut previous = [0usize; 6];
    let mut visited = [false; 6];
    costs[0] = 0.;
    for _ in 0..6 {
        let Some(index) = (0..6)
            .filter(|&i| !visited[i])
            .min_by(|&a, &b| costs[a].total_cmp(&costs[b]))
        else {
            break;
        };
        if !costs[index].is_finite() || index == 1 {
            break;
        }
        visited[index] = true;
        for next in 1..6 {
            if visited[next] || blocked(nodes[index], nodes[next], extent) {
                continue;
            }
            let cost = costs[index] + nodes[index].distance(nodes[next]);
            if cost < costs[next] {
                costs[next] = cost;
                previous[next] = index;
            }
        }
    }
    if !costs[1].is_finite() {
        return goal;
    }
    let mut next = 1;
    for _ in 0..6 {
        if previous[next] == 0 || nodes[previous[next]].distance(start) < 0.12 {
            break;
        }
        next = previous[next];
    }
    let point = center + x * nodes[next].x + z * nodes[next].y;
    [point.x, goal[1], point.y]
}

fn blocked(start: Vec2, end: Vec2, extent: Vec2) -> bool {
    let delta = end - start;
    let mut low = 0f32;
    let mut high = 1f32;
    for axis in 0..2 {
        let half = extent[axis] - 0.001;
        if delta[axis].abs() < 0.00001 {
            if start[axis].abs() >= half {
                return false;
            }
            continue;
        }
        let a = (-half - start[axis]) / delta[axis];
        let b = (half - start[axis]) / delta[axis];
        low = low.max(a.min(b));
        high = high.min(a.max(b));
        if low >= high {
            return false;
        }
    }
    low < 1. && high > 0.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opened_hangar_leaf_requires_a_clearance_route() {
        let pose = Transform {
            position: [98., 0., 103.8],
            rotation: [0., 100., 0.],
            ..Default::default()
        };
        let collider = Collider {
            center: [1., 1.3, 0.],
            half_extents: [1., 1.3, 0.12],
            ..Default::default()
        };
        let goal = [98., 0., 106.];
        let next = around_box([97.887, 0., 101.305], goal, goal, &pose, &collider, 0.4);
        assert!(
            next[0] > 98.,
            "the route must clear the swung leaf: {next:?}"
        );
        assert_ne!(next, goal);
        assert_eq!(
            around_box([100., 0., 100.], goal, goal, &pose, &collider, 0.4),
            goal
        );
    }
}
