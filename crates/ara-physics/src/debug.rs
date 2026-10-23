//! Presentation-only streams. Generating these never changes the simulation.
use crate::{BodyHandle, PhysicsPose, ShapeHandle, ShapePart};

/// Immutable local-space collision geometry; the renderer caches it by shape handle.
#[derive(Debug, Clone)]
pub enum DebugGeometry {
    /// Axis-aligned local box.
    Box(
        /// Half extents in meters.
        [f32; 3],
    ),
    /// Sphere in local space.
    Sphere(
        /// Radius in meters.
        f32,
    ),
    /// Upright capsule.
    Capsule {
        /// Radius in meters.
        radius: f32,
        /// Cylindrical half height.
        half_height: f32,
    },
    /// Upright cylinder.
    Cylinder {
        /// Radius in meters.
        radius: f32,
        /// Half height in meters.
        half_height: f32,
    },
    /// Hull input point cloud; presentation may draw its convex envelope.
    ConvexHull(
        /// Local-space input vertices.
        Vec<[f32; 3]>,
    ),
    /// Triangle mesh or triangulated heightfield.
    Mesh {
        /// Local-space positions.
        vertices: Vec<[f32; 3]>,
        /// Flattened triangle indices.
        indices: Vec<u32>,
    },
    /// Existing shapes positioned in a compound's local space.
    Compound(
        /// Local transforms and shape handles.
        Vec<ShapePart>,
    ),
}

/// Typed draw request, suitable for a renderer command stream without linking Jolt.
#[derive(Debug, Clone, Copy)]
pub enum DebugPrimitive {
    /// Draw cooked collision geometry at its current body origin.
    Shape {
        /// Body identity.
        body: BodyHandle,
        /// World-owned geometry cache key.
        shape: ShapeHandle,
        /// World transform.
        pose: PhysicsPose,
        /// Render asleep bodies distinctly.
        sleeping: bool,
    },
    /// Draw a contact normal.
    Contact {
        /// Contact point.
        point: [f32; 3],
        /// Normal from body A to B.
        normal: [f32; 3],
        /// Trigger contacts can use a distinct color.
        sensor: bool,
    },
    /// Draw an upright capsule and its support state.
    Character {
        /// Feet position.
        feet: [f32; 3],
        /// Actual capsule height, including crouch.
        height: f32,
        /// Capsule radius.
        radius: f32,
        /// Walkable support flag.
        grounded: bool,
    },
    /// Draw a wheel center and radius.
    Wheel {
        /// World-space wheel center.
        center: [f32; 3],
        /// Wheel radius.
        radius: f32,
    },
    /// Draw world-space constraint anchors and primary axis.
    Joint {
        /// First anchor.
        anchor_a: [f32; 3],
        /// Second anchor.
        anchor_b: [f32; 3],
        /// Constraint primary direction.
        axis: [f32; 3],
    },
}
