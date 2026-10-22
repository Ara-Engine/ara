//! Built-in engine components.
//!
//! These are the authoring declarations of components the engine itself understands.
//! Subsystem crates will own their components as they land (render, physics, audio);
//! until then this crate holds the ones the kernel commands need. Register them with
//! [`register`].

use ara_schema::{Schema, SchemaDb};

/// Position, rotation and scale of an entity relative to its parent.
#[derive(Debug, Clone, Copy, PartialEq, Schema)]
#[ara(component, module = "core")]
pub struct Transform {
    /// Position relative to the parent.
    #[ara(
        unit = Meters,
        example = [0, 1, 0],
        replicate(audience = Relevant, quantize = 0.001, interpolate = Linear, rollback)
    )]
    pub position: [f32; 3],
    /// Rotation relative to the parent, as XYZ Euler angles.
    #[ara(
        unit = Degrees,
        example = [0, 90, 0],
        replicate(audience = Relevant, quantize = 0.01, interpolate = Slerp, rollback)
    )]
    pub rotation: [f32; 3],
    /// Scale relative to the parent; `[1, 1, 1]` is unscaled.
    #[ara(default = [1, 1, 1], example = [2, 2, 2])]
    pub scale: [f32; 3],
}

impl Default for Transform {
    /// The identity transform, matching the schema defaults.
    fn default() -> Self {
        Transform {
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

/// Registers every built-in component.
pub fn register(db: &mut SchemaDb) {
    db.register::<Transform>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_default_matches_schema_default() {
        let mut db = SchemaDb::new();
        register(&mut db);
        assert!(db.verify().is_empty());
        let t = db.get("Transform").expect("registered");
        let scale = t.field("scale").expect("field");
        assert_eq!(db.field_default(scale), serde_json::json!([1, 1, 1]));
        assert_eq!(Transform::default().scale, [1.0; 3]);
    }
}
