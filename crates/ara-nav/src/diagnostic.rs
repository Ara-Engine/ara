//! Structured reachability evidence for playtest agents and `ara why`.
use crate::{NavError, NavMesh, QueryFilter, distance};
use ara_diag::{Code, Diagnostic};
use serde::{Deserialize, Serialize};

/// Evidence explaining why a requested movement cannot complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReachabilityReason {
    /// Both endpoints share a complete permitted corridor.
    Reachable,
    /// Start cannot project onto permitted mesh within the search extents.
    StartOutside,
    /// Goal cannot project onto any nearby mesh.
    GoalOutside,
    /// A complete corridor exists only when this area is allowed.
    BlockingArea {
        /// First forbidden area on the unrestricted corridor.
        area: u8,
    },
    /// An authored disabled connection plausibly bridges the disconnected regions.
    DisabledLink {
        /// Candidate link ID; an actionable suggestion, not proof of reachability.
        id: u32,
    },
    /// Disconnected islands; a connection or geometry change is needed.
    MissingLink,
    /// Query capacity was exhausted; increase capacity before concluding disconnection.
    Capacity,
}

/// Machine-readable explanation and a common kernel diagnostic for display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reachability {
    /// Structured cause.
    pub reason: ReachabilityReason,
    /// Closest point reached by a permitted partial corridor, when one exists.
    pub nearest_reachable: Option<[f32; 3]>,
    /// Common diagnostic payload (code 7100; nav-local reservation).
    pub diagnostic: Diagnostic,
}
fn report(reason: ReachabilityReason, nearest: Option<[f32; 3]>, message: &str) -> Reachability {
    let diagnostic = if reason == ReachabilityReason::Reachable {
        Diagnostic::info(Code(7100), message)
    } else {
        Diagnostic::warning(Code(7100), message)
    };
    Reachability {
        reason,
        nearest_reachable: nearest,
        diagnostic,
    }
}
/// Explains reachability with endpoint projections, a filtered corridor and an
/// unrestricted corridor. Disabled-link suggestions are explicitly candidates.
pub fn why_unreachable(
    mesh: &NavMesh,
    start: [f32; 3],
    goal: [f32; 3],
    filter: &QueryFilter,
) -> Result<Reachability, NavError> {
    let mut query = mesh.query()?;
    if query.nearest(start, filter)?.is_none() {
        return Ok(report(
            ReachabilityReason::StartOutside,
            None,
            "Start has no permitted nearby navigation polygon",
        ));
    }
    let all = QueryFilter::default();
    if query.nearest(goal, &all)?.is_none() {
        return Ok(report(
            ReachabilityReason::GoalOutside,
            None,
            "Goal is outside the baked navigation surface",
        ));
    }
    let filtered = query.path(start, goal, filter).ok();
    let nearest = filtered.as_ref().map(|p| p.reachable);
    if let Some(path) = &filtered {
        if path.truncated {
            return Ok(report(
                ReachabilityReason::Capacity,
                nearest,
                "Path search exhausted its configured capacity",
            ));
        }
        if path.complete {
            // A projected forbidden destination can otherwise look reachable.
            if let Some(goal_poly) = query.nearest(goal, &all)?
                && let Some(area) = query.polygon_area(goal_poly.polygon)
                && !filter.allows(area)
            {
                return Ok(report(
                    ReachabilityReason::BlockingArea { area },
                    nearest,
                    "Goal lies in a forbidden navigation area",
                ));
            }
            return Ok(report(
                ReachabilityReason::Reachable,
                nearest,
                "Goal has a complete permitted corridor",
            ));
        }
    }
    if let Ok(path) = query.path(start, goal, &all) {
        if path.truncated {
            return Ok(report(
                ReachabilityReason::Capacity,
                nearest,
                "Unrestricted search exhausted its configured capacity",
            ));
        }
        if path.complete {
            for corner in &path.corners {
                if let Some(area) = query.polygon_area(corner.polygon)
                    && !filter.allows(area)
                {
                    return Ok(report(
                        ReachabilityReason::BlockingArea { area },
                        nearest,
                        "The connecting corridor crosses a forbidden area",
                    ));
                }
            }
        }
    }
    if let Some(reachable) = nearest {
        for link in mesh.links().iter().filter(|l| !l.enabled) {
            let near = distance(link.start, reachable) < link.radius + 2.0
                && distance(link.end, goal) < distance(reachable, goal);
            let reverse = link.bidirectional
                && distance(link.end, reachable) < link.radius + 2.0
                && distance(link.start, goal) < distance(reachable, goal);
            if near || reverse {
                return Ok(report(
                    ReachabilityReason::DisabledLink { id: link.id },
                    nearest,
                    "A disabled authored link is a candidate for bridging these islands",
                ));
            }
        }
    }
    Ok(report(
        ReachabilityReason::MissingLink,
        nearest,
        "Navigation islands are disconnected; inspect missing links or collision geometry",
    ))
}
