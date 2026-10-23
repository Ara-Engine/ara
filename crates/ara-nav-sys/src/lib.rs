//! Private, pinned Recast/Detour C ABI; safe ownership lives in `ara-nav`.
//!
//! All pointer arguments must be live, aligned and valid for their declared
//! counts. Calls borrow buffers only. Mesh mutation is exclusive; queries have
//! private scratch and borrow a mesh which must outlive them. Match each returned
//! owned handle with its `*_free` function. Counts must fit a positive C int.
#![warn(missing_docs)]
use std::ffi::c_void;

/// Validated Recast tile bake settings (Y-up metres).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Config {
    /// Core tile lower bound.
    pub min: [f32; 3],
    /// Core tile upper bound.
    pub max: [f32; 3],
    /// Horizontal voxel size.
    pub cell_size: f32,
    /// Vertical voxel size.
    pub cell_height: f32,
    /// Agent collision radius.
    pub radius: f32,
    /// Required agent headroom.
    pub height: f32,
    /// Maximum step height.
    pub climb: f32,
    /// Maximum slope in degrees.
    pub slope: f32,
    /// Maximum contour simplification error in voxels.
    pub error: f32,
    /// Interior tile width in voxels.
    pub tile_size: i32,
    /// Tile grid X.
    pub tile_x: i32,
    /// Tile grid Z.
    pub tile_z: i32,
}
/// One off-mesh connection owned by the tile containing its start.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Link {
    /// Start position.
    pub start: [f32; 3],
    /// Destination position.
    pub end: [f32; 3],
    /// Endpoint attachment radius.
    pub radius: f32,
    /// Nonzero authored ID.
    pub id: u32,
    /// Traversal area 0..15.
    pub area: u8,
    /// Zero for one-way, one for bidirectional.
    pub bidirectional: u8,
}
/// World-space temporary obstacle box.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Box3 {
    /// Lower bound.
    pub min: [f32; 3],
    /// Upper bound.
    pub max: [f32; 3],
    /// Area 0..15 relabels walkable spans inside; 255 carves them out.
    pub area: u8,
}
/// Detour area costs, polygon flag masks and runtime-blocked links.
#[repr(C)]
pub struct Filter {
    /// Finite traversal costs of at least one.
    pub costs: [f32; 16],
    /// Allowed area bits.
    pub include: u16,
    /// Forbidden area bits.
    pub exclude: u16,
    /// Ascending off-mesh link IDs that must not be traversed (may be null).
    pub blocked: *const u32,
    /// Number of entries in `blocked`.
    pub blocked_count: i32,
}
/// One Detour funnel corner.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Corner {
    /// World-space position.
    pub position: [f32; 3],
    /// Opaque mesh-local polygon reference.
    pub polygon: u32,
    /// Detour straight-path flags.
    pub flags: u8,
}
/// Transient local-avoidance input/output, reset from ECS on each call.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Agent {
    /// Current position; overwritten with the constrained result.
    pub position: [f32; 3],
    /// Current velocity; overwritten with the solver result.
    pub velocity: [f32; 3],
    /// Desired path-following velocity.
    pub desired: [f32; 3],
    /// Collision radius.
    pub radius: f32,
    /// Collision height.
    pub height: f32,
    /// Maximum speed.
    pub speed: f32,
    /// Maximum acceleration.
    pub acceleration: f32,
    /// Allowed area bits.
    pub include: u16,
    /// Forbidden area bits.
    pub exclude: u16,
}
/// One unlinked polygon edge: the boundary of the walkable surface.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Wall {
    /// First endpoint.
    pub a: [f32; 3],
    /// Second endpoint.
    pub b: [f32; 3],
}
/// One polygon edge lying on a tile border (a Detour portal edge).
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Border {
    /// First endpoint.
    pub a: [f32; 3],
    /// Second endpoint.
    pub b: [f32; 3],
    /// Owning tile X.
    pub tile_x: i32,
    /// Owning tile Z.
    pub tile_z: i32,
    /// Portal direction: 0 = +X, 2 = +Z, 4 = -X, 6 = -Z.
    pub side: u8,
    /// One when a loaded neighbouring tile is stitched to this edge.
    pub linked: u8,
    /// Area of the owning polygon.
    pub area: u8,
}
/// Renderer-independent triangle emitted from the runtime mesh.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct DebugTriangle {
    /// First vertex.
    pub a: [f32; 3],
    /// Second vertex.
    pub b: [f32; 3],
    /// Third vertex.
    pub c: [f32; 3],
    /// Semantic area ID.
    pub area: u8,
}

// SAFETY: these declarations match the owned shim/nav.h POD layouts and C ABI;
// callers must uphold the module's buffer, ownership and exclusivity contracts.
unsafe extern "C" {
    /// Bakes a tile from validated geometry; zero succeeds and writes an owned handle.
    pub fn an_bake(
        c: *const Config,
        v: *const f32,
        nv: i32,
        t: *const i32,
        a: *const u8,
        nt: i32,
        l: *const Link,
        nl: i32,
        b: *const Box3,
        nb: i32,
        out: *mut *mut c_void,
    ) -> i32;
    /// Borrows a live tile's native byte buffer until that tile is freed.
    pub fn an_tile_bytes(tile: *const c_void, size: *mut i32) -> *const u8;
    /// Releases one owned bake-result handle.
    pub fn an_tile_free(tile: *mut c_void);
    /// Allocates a tiled mesh; returns null on allocation/capacity failure.
    pub fn an_mesh_new(origin: *const f32, width: f32, count: i32) -> *mut c_void;
    /// Releases a mesh after all its borrowed queries/crowds are gone.
    pub fn an_mesh_free(mesh: *mut c_void);
    /// Validates and copies one private-format tile; requires exclusive mesh access.
    pub fn an_mesh_add(mesh: *mut c_void, data: *const u8, size: i32) -> i32;
    /// Allocates independent query scratch borrowing a live immutable mesh.
    pub fn an_query_new(mesh: *const c_void, nodes: i32) -> *mut c_void;
    /// Releases one owned query handle and its reusable buffers.
    pub fn an_query_free(q: *mut c_void);
    /// Projects a point; returns zero when no permitted polygon is near.
    pub fn an_nearest(
        q: *mut c_void,
        p: *const f32,
        ext: *const f32,
        f: *const Filter,
        out: *mut f32,
    ) -> u32;
    /// Finds an A* corridor and funnel corners, reporting Detour status/detail bits.
    pub fn an_path(
        q: *mut c_void,
        a: *const f32,
        b: *const f32,
        ext: *const f32,
        f: *const Filter,
        corners: *mut Corner,
        cap: i32,
        count: *mut i32,
        complete: *mut u8,
        reachable: *mut f32,
    ) -> u32;
    /// Raycasts along the mesh (off-mesh connections are not followed).
    pub fn an_raycast(
        q: *mut c_void,
        a: *const f32,
        b: *const f32,
        ext: *const f32,
        f: *const Filter,
        t: *mut f32,
        normal: *mut f32,
    ) -> u32;
    /// Returns an area's ID, or -1 for an invalid/stale polygon reference.
    pub fn an_polygon_area(q: *mut c_void, polygon: u32) -> i32;
    /// Tests a surface segment and verifies it ends on the destination's floor.
    pub fn an_visible(
        q: *mut c_void,
        a: *const f32,
        b: *const f32,
        ext: *const f32,
        f: *const Filter,
    ) -> i32;
    /// Counts runtime debug triangles without allocating.
    pub fn an_debug_count(mesh: *const c_void) -> i32;
    /// Writes at most cap debug triangles to the provided output buffer.
    pub fn an_debug(mesh: *const c_void, out: *mut DebugTriangle, cap: i32) -> i32;
    /// Runs transient DetourCrowd avoidance from complete ECS state, in input order.
    pub fn an_crowd_step(mesh: *const c_void, agents: *mut Agent, count: i32, dt: f32) -> i32;
    /// Writes at most cap boundary edges; returns the total count.
    pub fn an_walls(mesh: *const c_void, out: *mut Wall, cap: i32) -> i32;
    /// Writes at most cap tile-border edges; returns the total count.
    pub fn an_borders(mesh: *const c_void, out: *mut Border, cap: i32) -> i32;
}
