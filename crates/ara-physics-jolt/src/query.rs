use crate::{JoltBackend, convert::*};
use ara_jolt_sys as sys;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn query_batch(&self, queries: &[Query], out: &mut QueryResults) -> Result<()> {
        let mut raw = Vec::with_capacity(queries.len());
        for query in queries {
            let (kind, shape, pose, displacement, filter) = match *query {
                Query::Ray {
                    origin,
                    displacement,
                    filter,
                } => (
                    0,
                    None,
                    PhysicsPose {
                        position: origin,
                        ..Default::default()
                    },
                    displacement,
                    filter,
                ),
                Query::Cast {
                    shape,
                    pose,
                    displacement,
                    filter,
                } => (1, Some(shape), pose, displacement, filter),
                Query::Overlap {
                    shape,
                    pose,
                    filter,
                } => (2, Some(shape), pose, [0.0; 3], filter),
            };
            if let Some(s) = shape {
                self.require_shape(s)?;
            }
            if let Some(b) = filter.ignore {
                self.require_body(b)?;
            }
            raw.push(sys::AraQuery {
                kind,
                shape: shape.map_or(sys::INVALID, ShapeHandle::raw),
                layer_mask: filter.layer_mask,
                sensors: u32::from(filter.sensors),
                ignore: filter.ignore.map_or(sys::INVALID, BodyHandle::raw),
                position: v(pose.position),
                rotation: q(pose.rotation),
                displacement: v(displacement),
            });
        }
        let count = Self::count(raw.len())?;
        let mut offsets = vec![0; raw.len() + 1];
        let mut capacity = out.hits.capacity().max(raw.len().saturating_mul(4)).max(64);
        loop {
            let mut hits = vec![sys::AraHit::default(); capacity];
            // SAFETY: offsets has count+1 elements; hits and queries have their
            // declared capacities. Native queries are read-only and synchronous.
            let ok = unsafe {
                sys::ara_queries(
                    self.ptr(),
                    raw.as_ptr(),
                    count,
                    hits.as_mut_ptr(),
                    Self::count(capacity)?,
                    offsets.as_mut_ptr(),
                )
            };
            if ok == 1 {
                let len = offsets.last().copied().unwrap_or(0) as usize;
                out.hits.clear();
                out.hits.extend(hits[..len].iter().copied().map(hit));
                out.offsets.clear();
                out.offsets.extend(offsets.into_iter().map(|n| n as usize));
                return Ok(());
            }
            let error = Self::error();
            if error.0 != "query output capacity exhausted" {
                return Err(error);
            }
            capacity = capacity
                .checked_mul(2)
                .filter(|n| *n <= 16_777_216)
                .ok_or(error)?;
        }
    }
}
