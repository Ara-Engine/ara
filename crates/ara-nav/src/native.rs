//! Audited ownership and lifetimes for the owned C shim; no pointers escape.
use crate::{BakeArtifact, BakeSettings, Bounds, Geometry, NavError, OffMeshLink, TileCoord};
use ara_nav_sys as sys;
use std::{ffi::c_void, ptr::NonNull};

pub(crate) fn bake(
    settings: &BakeSettings,
    coord: TileCoord,
    geometry: &Geometry,
    links: &[OffMeshLink],
    boxes: &[Bounds],
) -> Result<Vec<u8>, NavError> {
    if geometry.triangles.is_empty() {
        return Ok(Vec::new());
    }
    let bounds = settings.tile_bounds(coord);
    let c = sys::Config {
        min: bounds.min,
        max: bounds.max,
        cell_size: settings.cell_size,
        cell_height: settings.cell_height,
        radius: settings.agent.radius,
        height: settings.agent.height,
        climb: settings.agent.climb,
        slope: settings.agent.slope,
        error: settings.max_error,
        tile_size: i32::from(settings.tile_size),
        tile_x: coord.x,
        tile_z: coord.z,
    };
    let vertices: Vec<f32> = geometry.vertices.iter().flatten().copied().collect();
    let triangles: Vec<i32> = geometry
        .triangles
        .iter()
        .flatten()
        .map(|&i| i as i32)
        .collect();
    let links: Vec<_> = links
        .iter()
        .map(|l| sys::Link {
            start: l.start,
            end: l.end,
            radius: l.radius,
            id: l.id,
            area: l.area,
            bidirectional: u8::from(l.bidirectional),
        })
        .collect();
    let boxes: Vec<_> = boxes
        .iter()
        .map(|b| sys::Box3 {
            min: b.min,
            max: b.max,
        })
        .collect();
    let mut output = std::ptr::null_mut();
    // SAFETY: Baker validates finite settings and all indices; flattened C POD slices
    // have the declared element counts and remain borrowed for the entire call.
    let code = unsafe {
        sys::an_bake(
            &c,
            vertices.as_ptr(),
            geometry.vertices.len() as i32,
            triangles.as_ptr(),
            geometry.areas.as_ptr(),
            geometry.triangles.len() as i32,
            links.as_ptr(),
            links.len() as i32,
            boxes.as_ptr(),
            boxes.len() as i32,
            &mut output,
        )
    };
    if code != 0 {
        return Err(NavError::new(
            "bake",
            format!(
                "Recast failed at stage {code} for tile {},{}",
                coord.x, coord.z
            ),
        ));
    }
    let tile = NativeTile(
        NonNull::new(output)
            .ok_or_else(|| NavError::new("allocation", "Recast returned a null tile"))?,
    );
    let mut size = 0;
    // SAFETY: tile is an owned, live shim result; size is a writable C int.
    let bytes = unsafe { sys::an_tile_bytes(tile.0.as_ptr(), &mut size) };
    if size == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: the shim owns a buffer of exactly size bytes until tile drops.
    Ok(unsafe { std::slice::from_raw_parts(bytes, size as usize) }.to_vec())
}
struct NativeTile(NonNull<c_void>);
impl Drop for NativeTile {
    fn drop(&mut self) {
        // SAFETY: uniquely owned shim tile is released once with its matching allocator.
        unsafe { sys::an_tile_free(self.0.as_ptr()) };
    }
}

pub(crate) struct Mesh(NonNull<c_void>);
// SAFETY: mesh ownership can move between threads; Detour uses no thread-local data.
unsafe impl Send for Mesh {}
// SAFETY: after construction the mesh is immutable. Every query/crowd owns separate
// mutable scratch, and native queries do not mutate the referenced navmesh.
unsafe impl Sync for Mesh {}
impl Mesh {
    pub(crate) fn new(artifact: &BakeArtifact) -> Result<Self, NavError> {
        artifact.settings.validate()?;
        if artifact.tiles.len() > 8192 {
            return Err(NavError::new("capacity", "more than 8192 tiles"));
        }
        // SAFETY: validated finite grid settings; at least one slot, bounded count.
        let mesh = unsafe {
            sys::an_mesh_new(
                artifact.settings.origin.as_ptr(),
                artifact.settings.tile_width(),
                artifact.tiles.len().max(1) as i32,
            )
        };
        let mesh = Self(
            NonNull::new(mesh)
                .ok_or_else(|| NavError::new("allocation", "Detour mesh initialization failed"))?,
        );
        for tile in artifact.tiles.values().filter(|t| !t.bytes.is_empty()) {
            let size = i32::try_from(tile.bytes.len())
                .map_err(|_| NavError::new("capacity", "tile exceeds ABI limit"))?;
            // SAFETY: mesh is uniquely owned during construction. The shim validates
            // the complete private byte format and copies it before returning.
            let code = unsafe { sys::an_mesh_add(mesh.0.as_ptr(), tile.bytes.as_ptr(), size) };
            if code != 0 {
                return Err(NavError::new(
                    "invalidTile",
                    format!("Detour rejected tile {} (stage {code})", tile.key),
                ));
            }
        }
        Ok(mesh)
    }
    pub(crate) fn query(&self, nodes: i32) -> Result<Query<'_>, NavError> {
        // SAFETY: the native query borrows this live immutable mesh. Query's Rust
        // lifetime prevents it from outliving the mesh; nodes is bounded by caller.
        let q = unsafe { sys::an_query_new(self.0.as_ptr(), nodes) };
        Ok(Query {
            pointer: NonNull::new(q)
                .ok_or_else(|| NavError::new("allocation", "Detour query allocation failed"))?,
            _mesh: self,
            corners: Vec::new(),
        })
    }
    pub(crate) fn debug(&self) -> Vec<sys::DebugTriangle> {
        // SAFETY: read-only traversal of a valid live mesh.
        let count = unsafe { sys::an_debug_count(self.0.as_ptr()) };
        let mut triangles = vec![sys::DebugTriangle::default(); count as usize];
        // SAFETY: buffer is initialized and has capacity for exactly count results.
        unsafe { sys::an_debug(self.0.as_ptr(), triangles.as_mut_ptr(), count) };
        triangles
    }
    pub(crate) fn crowd_step(&self, agents: &mut [sys::Agent], dt: f32) -> Result<(), NavError> {
        if agents.is_empty() {
            return Ok(());
        }
        // SAFETY: the caller validates all values, masks/counts and dt. The shim
        // owns transient crowd scratch and retains neither mesh nor agent pointers.
        let code = unsafe {
            sys::an_crowd_step(
                self.0.as_ptr(),
                agents.as_mut_ptr(),
                agents.len() as i32,
                dt,
            )
        };
        if code != 0 {
            return Err(NavError::new(
                "crowd",
                format!("Detour crowd step failed at stage {code}"),
            ));
        }
        Ok(())
    }
}
impl Drop for Mesh {
    fn drop(&mut self) {
        // SAFETY: no borrowed query can outlive this uniquely owned allocation.
        unsafe { sys::an_mesh_free(self.0.as_ptr()) };
    }
}

pub(crate) struct Query<'a> {
    pointer: NonNull<c_void>,
    _mesh: &'a Mesh,
    corners: Vec<sys::Corner>,
}
pub(crate) struct PathResult<'a> {
    pub(crate) corners: &'a [sys::Corner],
    pub(crate) complete: bool,
    pub(crate) reachable: [f32; 3],
    pub(crate) status: u32,
}
impl Query<'_> {
    pub(crate) fn nearest(
        &mut self,
        point: [f32; 3],
        ext: [f32; 3],
        filter: &sys::Filter,
    ) -> Option<(u32, [f32; 3])> {
        let mut out = [0.0; 3];
        // SAFETY: query scratch is exclusively borrowed; all vectors have 3 finite
        // elements and the validated filter lives through this synchronous call.
        let polygon = unsafe {
            sys::an_nearest(
                self.pointer.as_ptr(),
                point.as_ptr(),
                ext.as_ptr(),
                filter,
                out.as_mut_ptr(),
            )
        };
        (polygon != 0).then_some((polygon, out))
    }
    pub(crate) fn path(
        &mut self,
        a: [f32; 3],
        b: [f32; 3],
        ext: [f32; 3],
        filter: &sys::Filter,
        capacity: usize,
    ) -> Result<PathResult<'_>, NavError> {
        self.corners.resize(capacity, sys::Corner::default());
        let mut count = 0;
        let mut complete = 0;
        let mut reachable = [0.0; 3];
        // SAFETY: exclusive query, valid 3-element inputs and capacity-sized output
        // buffer. The shim never writes more than capacity initialized POD entries.
        let status = unsafe {
            sys::an_path(
                self.pointer.as_ptr(),
                a.as_ptr(),
                b.as_ptr(),
                ext.as_ptr(),
                filter,
                self.corners.as_mut_ptr(),
                capacity as i32,
                &mut count,
                &mut complete,
                reachable.as_mut_ptr(),
            )
        };
        if status & (1 << 31) != 0 {
            return Err(NavError::new(
                "noPath",
                format!("Detour path status {status:#x}"),
            ));
        }
        Ok(PathResult {
            corners: &self.corners[..count as usize],
            complete: complete != 0,
            reachable,
            status,
        })
    }
    pub(crate) fn raycast(
        &mut self,
        a: [f32; 3],
        b: [f32; 3],
        ext: [f32; 3],
        filter: &sys::Filter,
    ) -> Result<(f32, [f32; 3]), NavError> {
        let mut t = 0.0;
        let mut normal = [0.0; 3];
        // SAFETY: exclusive live query and validated 3-element input/output arrays.
        let status = unsafe {
            sys::an_raycast(
                self.pointer.as_ptr(),
                a.as_ptr(),
                b.as_ptr(),
                ext.as_ptr(),
                filter,
                &mut t,
                normal.as_mut_ptr(),
            )
        };
        if status & (1 << 31) != 0 {
            return Err(NavError::new(
                "noStart",
                "raycast start is outside the navmesh",
            ));
        }
        Ok((t.min(1.0), normal))
    }
    pub(crate) fn area(&mut self, polygon: u32) -> Option<u8> {
        // SAFETY: native getPolyArea validates arbitrary/stale polygon references.
        let area = unsafe { sys::an_polygon_area(self.pointer.as_ptr(), polygon) };
        u8::try_from(area).ok()
    }
    pub(crate) fn visible(
        &mut self,
        a: [f32; 3],
        b: [f32; 3],
        ext: [f32; 3],
        filter: &sys::Filter,
    ) -> bool {
        // SAFETY: exclusive live query, validated fixed-size points and filter;
        // the shim owns scratch and verifies visibility on the destination floor.
        unsafe {
            sys::an_visible(
                self.pointer.as_ptr(),
                a.as_ptr(),
                b.as_ptr(),
                ext.as_ptr(),
                filter,
            ) != 0
        }
    }
}
impl Drop for Query<'_> {
    fn drop(&mut self) {
        // SAFETY: uniquely owned query scratch is freed before its borrowed mesh.
        unsafe { sys::an_query_free(self.pointer.as_ptr()) };
    }
}
